//! Core data types shared across the crate. Storage-agnostic: the `Thesaurus`
//! trait speaks in these structs so another backend could produce the same shapes.

use serde::Serialize;

/// The result of a successful lookup.
#[derive(Debug, Clone, Serialize)]
pub struct Lookup {
    /// The word as the user typed it.
    pub query: String,
    /// True when no exact entry existed and the results come from base form(s).
    pub lemmatized: bool,
    /// One entry per matching headword (several only when an inflected form maps
    /// to more than one base, e.g. `saw` → `saw`, `see`).
    pub entries: Vec<Entry>,
}

/// One headword plus its senses.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub word: String,
    pub senses: Vec<Sense>,
}

/// One meaning of a word for a given part of speech.
#[derive(Debug, Clone, Serialize)]
pub struct Sense {
    /// Part of speech: `n`, `v`, `adj`, `adv`.
    pub pos: String,
    /// 1 = most common sense.
    pub num: i64,
    /// Short disambiguating label (hypernym or attribute), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub synonyms: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub antonyms: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Related>,
}

/// A group of related words of one kind (`hypernym`, `similar`, `see also`).
#[derive(Debug, Clone, Serialize)]
pub struct Related {
    pub kind: String,
    pub words: Vec<String>,
}

/// A spellcheck candidate produced when a lookup misses.
#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub word: String,
    /// Lower is closer (Damerau-Levenshtein with tie-breakers; see `suggest.rs`).
    pub score: usize,
}

impl Lookup {
    /// Drop senses that have nothing left to show (after `--no-*` pruning).
    pub fn prune(&mut self) {
        for e in &mut self.entries {
            e.senses.retain(|s| {
                !(s.synonyms.is_empty() && s.antonyms.is_empty() && s.related.is_empty())
            });
        }
        self.entries.retain(|e| !e.senses.is_empty());
    }
}
