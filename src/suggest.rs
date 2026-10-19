//! Spellcheck: gather candidate words cheaply from SQL, then rank in Rust.
//!
//! Stage 1 (SQL): words that share the query's Metaphone code (sound-alikes) or
//! sit in a nearby length band under the same first letter (typo-alikes).
//! Stage 2 (Rust): score by Damerau-Levenshtein distance with small tie-breakers
//! and return the closest `n`.

use std::collections::HashMap;

use anyhow::Result;
use rphonetic::{Encoder, Metaphone};
use rusqlite::Connection;

use crate::db;
use crate::model::Suggestion;

pub fn suggest(conn: &Connection, word: &str, lang: &str, n: usize) -> Result<Vec<Suggestion>> {
    let key = db::word_key(word);
    if key.is_empty() {
        return Ok(Vec::new());
    }

    // word_key -> display word (dedups across the two queries).
    let mut candidates: HashMap<String, String> = HashMap::new();

    // Stage 1a: Metaphone sound-alikes.
    let metaphone = Metaphone::default().encode(&key);
    if !metaphone.is_empty() {
        let mut stmt =
            conn.prepare("SELECT word, word_key FROM words WHERE lang = ?1 AND metaphone = ?2")?;
        let rows = stmt.query_map((lang, &metaphone), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (word, wkey) = row?;
            candidates.entry(wkey).or_insert(word);
        }
    }

    // Stage 1b: typo-alikes — same first letter, similar length.
    let first = key.chars().next().unwrap().to_string();
    let len = key.chars().count() as i64;
    let mut stmt = conn.prepare(
        "SELECT word, word_key FROM words \
         WHERE lang = ?1 AND substr(word_key, 1, 1) = ?2 \
         AND length(word_key) BETWEEN ?3 AND ?4 LIMIT 5000",
    )?;
    let rows = stmt.query_map((lang, &first, len - 2, len + 2), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (word, wkey) = row?;
        candidates.entry(wkey).or_insert(word);
    }

    // Stage 2: rank, excluding the query itself.
    let mut scored: Vec<Suggestion> = candidates
        .into_iter()
        .filter(|(wkey, _)| wkey != &key)
        .map(|(wkey, word)| Suggestion {
            score: score(&key, &wkey),
            word,
        })
        .collect();

    scored.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.word.cmp(&b.word)));
    scored.truncate(n);
    Ok(scored)
}

/// Lower is closer. Distance dominates; ties broken by first-letter mismatch and
/// length difference so equally-distant words order sensibly.
fn score(query: &str, candidate: &str) -> usize {
    let dist = strsim::damerau_levenshtein(query, candidate);
    let first_penalty = match (query.chars().next(), candidate.chars().next()) {
        (Some(a), Some(b)) if a == b => 0,
        _ => 1,
    };
    let len_penalty =
        (query.chars().count() as isize - candidate.chars().count() as isize).unsigned_abs();
    dist * 100 + first_penalty * 10 + len_penalty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closer_words_score_lower() {
        assert!(score("large", "large") < score("large", "larch"));
        assert!(score("large", "larch") < score("large", "zebra"));
    }

    #[test]
    fn transposition_counts_as_one_edit() {
        assert!(score("hte", "the") < score("hte", "cat"));
    }
}
