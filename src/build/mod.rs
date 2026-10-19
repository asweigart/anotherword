//! Database construction from external sources.
//!
//! `--build` runs each registered [`SourceImporter`] against a single
//! transaction, then derives the Metaphone index. Adding a language or a new
//! dataset is a matter of adding another importer here — nothing in the query
//! path changes.

pub mod download;
pub mod wordnet;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rphonetic::{Encoder, Metaphone};
use rusqlite::Connection;

use crate::db;
use crate::model::Related;

/// A source of thesaurus data. Each importer downloads what it needs into
/// `cache_dir` (idempotently) and writes rows through the [`DbWriter`].
pub trait SourceImporter {
    fn name(&self) -> &str;
    fn import(&self, writer: &mut DbWriter, cache_dir: &Path) -> Result<()>;
}

fn importers() -> Vec<Box<dyn SourceImporter>> {
    vec![Box::new(wordnet::WordNetImporter::new("en"))]
}

/// Build (or rebuild) the database at `db_path`. With `force`, an existing DB is
/// discarded first. Cached source downloads live in a `sources/` dir beside it.
pub fn build_database(db_path: &Path, force: bool) -> Result<()> {
    if force && db_path.exists() {
        std::fs::remove_file(db_path)
            .with_context(|| format!("removing existing DB at {}", db_path.display()))?;
    }

    let cache_dir = cache_dir_for(db_path);
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("creating source cache dir {}", cache_dir.display()))?;

    let mut conn = db::create(db_path)?;

    {
        let tx = conn.transaction()?;
        {
            let mut writer = DbWriter::new(&tx);
            writer.insert_language("en", "English")?;
            for importer in importers() {
                eprintln!("anotherword: importing {}...", importer.name());
                importer
                    .import(&mut writer, &cache_dir)
                    .with_context(|| format!("importing {}", importer.name()))?;
            }
        }
        eprintln!("anotherword: building phonetic index...");
        build_phonetics(&tx)?;
        tx.commit()?;
    }

    // Reclaim free pages / defragment and refresh planner stats.
    conn.execute_batch("VACUUM; ANALYZE;").ok();
    eprintln!("anotherword: done.");
    Ok(())
}

fn cache_dir_for(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("sources")
}

/// Fill `words.metaphone` (used by spellcheck), then index it. The index is
/// created *after* the bulk update so it isn't maintained per-row.
fn build_phonetics(conn: &Connection) -> Result<()> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, word_key FROM words")?;
        let collected = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        collected
    };
    let encoder = Metaphone::default();
    let mut update = conn.prepare("UPDATE words SET metaphone = ?2 WHERE id = ?1")?;
    for (id, key) in rows {
        let code = encoder.encode(&key);
        if !code.is_empty() {
            update.execute((id, &code))?;
        }
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_words_meta ON words(metaphone)")?;
    Ok(())
}

/// Insert helper used by importers. Caches word ids in memory so repeated senses
/// for the same headword don't re-query.
pub struct DbWriter<'a> {
    conn: &'a Connection,
    cache: HashMap<(String, String), i64>,
}

impl<'a> DbWriter<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            cache: HashMap::new(),
        }
    }

    pub fn insert_language(&self, code: &str, name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO languages(code, name) VALUES(?1, ?2)",
            (code, name),
        )?;
        Ok(())
    }

    /// Get-or-create the word row and return its id.
    pub fn word_id(&mut self, lang: &str, word: &str) -> Result<i64> {
        let key = db::word_key(word);
        let ck = (lang.to_string(), key.clone());
        if let Some(&id) = self.cache.get(&ck) {
            return Ok(id);
        }
        self.conn
            .prepare_cached("INSERT OR IGNORE INTO words(lang, word, word_key) VALUES(?1, ?2, ?3)")?
            .execute((lang, word, &key))?;
        let id: i64 = self
            .conn
            .prepare_cached("SELECT id FROM words WHERE lang = ?1 AND word_key = ?2 LIMIT 1")?
            .query_row((lang, &key), |r| r.get(0))?;
        self.cache.insert(ck, id);
        Ok(id)
    }

    /// Add one sense to a word (creating the word if needed). A sense with no
    /// synonyms, antonyms, or related words has nothing to show and is skipped,
    /// so such words never become headwords.
    #[allow(clippy::too_many_arguments)]
    pub fn add_sense(
        &mut self,
        lang: &str,
        word: &str,
        pos: &str,
        sense_num: i64,
        hint: Option<&str>,
        synonyms: &[String],
        antonyms: &[String],
        related: &[Related],
    ) -> Result<()> {
        if synonyms.is_empty() && antonyms.is_empty() && related.is_empty() {
            return Ok(());
        }
        let id = self.word_id(lang, word)?;
        let join = |v: &[String]| (!v.is_empty()).then(|| v.join(", "));
        self.conn
            .prepare_cached(
                "INSERT INTO senses(word_id, pos, sense_num, hint, synonyms, antonyms, related) \
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?
            .execute((
                id,
                pos,
                sense_num,
                hint,
                join(synonyms),
                join(antonyms),
                encode_related(related),
            ))?;
        Ok(())
    }

    pub fn add_exception(&self, lang: &str, pos: &str, form: &str, base: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT OR IGNORE INTO exceptions(lang, pos, form, base) VALUES(?1, ?2, ?3, ?4)",
            )?
            .execute((lang, pos, form, base))?;
        Ok(())
    }
}

/// `kind:a, b|kind:c` — the on-disk form of a sense's related words.
pub fn encode_related(related: &[Related]) -> Option<String> {
    let parts: Vec<String> = related
        .iter()
        .filter(|r| !r.words.is_empty())
        .map(|r| format!("{}:{}", r.kind, r.words.join(", ")))
        .collect();
    (!parts.is_empty()).then(|| parts.join("|"))
}
