//! The query-side abstraction. Everything above this (CLI, output) speaks to a
//! `Thesaurus`; the SQLite backend is the only implementor in v1.

use anyhow::Result;
use rusqlite::Connection;

use crate::db;
use crate::lemma;
use crate::model::{Entry, Lookup, Related, Sense, Suggestion};
use crate::suggest;

pub trait Thesaurus {
    /// Look up `word`: an exact entry if there is one, otherwise the entries for
    /// its base form(s). `None` if neither exists.
    fn lookup(&self, word: &str, lang: &str) -> Result<Option<Lookup>>;

    /// Up to `n` similarly spelled real words, closest first.
    fn suggest(&self, word: &str, lang: &str, n: usize) -> Result<Vec<Suggestion>>;
}

pub struct SqliteThesaurus {
    conn: Connection,
}

impl SqliteThesaurus {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn has_language(&self, lang: &str) -> bool {
        self.conn
            .query_row("SELECT 1 FROM languages WHERE code = ?1", [lang], |_| Ok(()))
            .is_ok()
    }

    /// Build an `Entry` for `key`, optionally restricted to one part of speech.
    /// `None` if the word is absent or has no senses in that part of speech.
    fn entry_for(&self, lang: &str, key: &str, pos: Option<&str>) -> Result<Option<Entry>> {
        let row = self
            .conn
            .query_row(
                "SELECT id, word FROM words WHERE lang = ?1 AND word_key = ?2 LIMIT 1",
                (lang, key),
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .ok();
        let Some((word_id, word)) = row else {
            return Ok(None);
        };

        let mut stmt = self.conn.prepare(
            "SELECT pos, sense_num, hint, synonyms, antonyms, related FROM senses \
             WHERE word_id = ?1 AND (?2 IS NULL OR pos = ?2) \
             ORDER BY CASE pos WHEN 'n' THEN 0 WHEN 'v' THEN 1 WHEN 'adj' THEN 2 ELSE 3 END, \
                      sense_num",
        )?;
        let senses = stmt
            .query_map((word_id, pos), |r| {
                Ok(Sense {
                    pos: r.get(0)?,
                    num: r.get(1)?,
                    hint: r.get(2)?,
                    synonyms: split_list(r.get::<_, Option<String>>(3)?.as_deref()),
                    antonyms: split_list(r.get::<_, Option<String>>(4)?.as_deref()),
                    related: decode_related(r.get::<_, Option<String>>(5)?.as_deref()),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        if senses.is_empty() {
            return Ok(None);
        }
        Ok(Some(Entry { word, senses }))
    }

    /// Irregular base forms for `form` from the exceptions table: `(pos, base)`.
    fn exception_bases(&self, lang: &str, form: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT pos, base FROM exceptions WHERE lang = ?1 AND form = ?2")?;
        let rows = stmt
            .query_map((lang, form), |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

impl Thesaurus for SqliteThesaurus {
    fn lookup(&self, word: &str, lang: &str) -> Result<Option<Lookup>> {
        let key = db::word_key(word);
        if key.is_empty() {
            return Ok(None);
        }

        // An exact entry always wins over a base form (`left` is a word itself).
        if let Some(entry) = self.entry_for(lang, &key, None)? {
            return Ok(Some(Lookup {
                query: word.to_string(),
                lemmatized: false,
                entries: vec![entry],
            }));
        }

        // Candidate (pos, base) pairs: irregular forms first, then regular rules.
        let mut candidates: Vec<(String, String)> = self.exception_bases(lang, &key)?;
        candidates.extend(
            lemma::candidates(&key)
                .into_iter()
                .map(|(p, b)| (p.to_string(), b)),
        );

        // Group by base so one headword is shown once, with all matching POS.
        let mut entries: Vec<Entry> = Vec::new();
        for (pos, base) in candidates {
            let Some(found) = self.entry_for(lang, &db::word_key(&base), Some(&pos))? else {
                continue;
            };
            match entries.iter_mut().find(|e| e.word == found.word) {
                Some(existing) => {
                    for s in found.senses {
                        let dup = existing.senses.iter().any(|x| x.pos == s.pos && x.num == s.num);
                        if !dup {
                            existing.senses.push(s);
                        }
                    }
                }
                None => entries.push(found),
            }
        }

        if entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(Lookup {
            query: word.to_string(),
            lemmatized: true,
            entries,
        }))
    }

    fn suggest(&self, word: &str, lang: &str, n: usize) -> Result<Vec<Suggestion>> {
        suggest::suggest(&self.conn, word, lang, n)
    }
}

fn split_list(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

/// Inverse of `build::encode_related`: `kind:a, b|kind:c`.
fn decode_related(raw: Option<&str>) -> Vec<Related> {
    raw.map(|s| {
        s.split('|')
            .filter_map(|part| part.split_once(':'))
            .map(|(kind, words)| Related {
                kind: kind.to_string(),
                words: split_list(Some(words)),
            })
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::DbWriter;

    fn s(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    /// A tiny in-memory fixture DB.
    fn fixture() -> SqliteThesaurus {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();
        {
            let mut w = DbWriter::new(&conn);
            w.insert_language("en", "English").unwrap();
            w.add_sense("en", "large", "adj", 1, Some("size"), &s(&["big"]), &s(&["small"]), &[])
                .unwrap();
            w.add_sense("en", "mouse", "n", 1, None, &s(&["computer mouse"]), &[], &[])
                .unwrap();
            w.add_sense("en", "ice cream", "n", 1, None, &s(&["frozen dessert"]), &[], &[])
                .unwrap();
            // A sense with nothing to show must not create a headword.
            w.add_sense("en", "empty", "n", 1, None, &[], &[], &[]).unwrap();
            w.add_exception("en", "n", "mice", "mouse").unwrap();
        }
        SqliteThesaurus::new(conn)
    }

    #[test]
    fn exact_lookup() {
        let t = fixture();
        let l = t.lookup("Large", "en").unwrap().unwrap();
        assert!(!l.lemmatized);
        let sense = &l.entries[0].senses[0];
        assert_eq!(sense.synonyms, vec!["big"]);
        assert_eq!(sense.antonyms, vec!["small"]);
        assert_eq!(sense.hint.as_deref(), Some("size"));
    }

    #[test]
    fn normalizes_phrases() {
        let t = fixture();
        assert!(t.lookup("Ice_Cream", "en").unwrap().is_some());
        assert!(t.lookup("  ice   cream ", "en").unwrap().is_some());
    }

    #[test]
    fn regular_and_irregular_base_forms() {
        let t = fixture();
        let l = t.lookup("larger", "en").unwrap().unwrap();
        assert!(l.lemmatized);
        assert_eq!(l.entries[0].word, "large");
        let l = t.lookup("mice", "en").unwrap().unwrap();
        assert_eq!(l.entries[0].word, "mouse");
    }

    #[test]
    fn base_form_respects_part_of_speech() {
        // "larges" would strip to "large" under the *noun* rule, but large is
        // only an adjective here, so it must not match.
        let t = fixture();
        assert!(t.lookup("larges", "en").unwrap().is_none());
    }

    #[test]
    fn misses_and_empty_senses() {
        let t = fixture();
        assert!(t.lookup("zzzz", "en").unwrap().is_none());
        assert!(t.lookup("empty", "en").unwrap().is_none());
    }

    #[test]
    fn related_round_trips() {
        let enc = vec![Related {
            kind: "similar".into(),
            words: s(&["huge", "enormous"]),
        }];
        let text = crate::build::encode_related(&enc);
        let dec = decode_related(text.as_deref());
        assert_eq!(dec[0].kind, "similar");
        assert_eq!(dec[0].words, vec!["huge", "enormous"]);
    }
}
