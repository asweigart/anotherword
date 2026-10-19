//! SQLite storage: path resolution and schema.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Filename of the database; cached source archives live beside it.
pub const DB_FILENAME: &str = "anotherword.sqlite";

/// Schema version stored in the `meta` table. Bump when the schema changes so a
/// stale DB can be detected and rebuilt.
pub const SCHEMA_VERSION: i64 = 1;

/// Where the DB lives, and how we found it (for `--where` reporting).
#[derive(Debug, Clone)]
pub struct DbLocation {
    pub path: PathBuf,
    pub source: LocationSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationSource {
    /// `ANOTHERWORD_DB` environment variable.
    EnvOverride,
    /// Next to the executable (portable mode).
    BesideExe,
    /// Platform data directory fallback.
    DataDir,
}

impl LocationSource {
    pub fn describe(self) -> &'static str {
        match self {
            LocationSource::EnvOverride => "ANOTHERWORD_DB override",
            LocationSource::BesideExe => "beside the executable (portable)",
            LocationSource::DataDir => "OS data directory",
        }
    }
}

/// Resolve the database path, portable-first (DESIGN.md §3).
///
/// Order: `ANOTHERWORD_DB` → `<dir-of-exe>/anotherword.sqlite` → OS data dir.
/// "Beside the exe" wins when the file already exists there or the directory is
/// writable; a read-only install directory falls through to the OS data dir.
pub fn resolve_location() -> Result<DbLocation> {
    if let Some(over) = std::env::var_os("ANOTHERWORD_DB") {
        return Ok(DbLocation {
            path: PathBuf::from(over),
            source: LocationSource::EnvOverride,
        });
    }

    if let Some(dir) = exe_dir() {
        let candidate = dir.join(DB_FILENAME);
        if candidate.exists() || dir_is_writable(&dir) {
            return Ok(DbLocation {
                path: candidate,
                source: LocationSource::BesideExe,
            });
        }
    }

    Ok(DbLocation {
        path: data_dir()?.join(DB_FILENAME),
        source: LocationSource::DataDir,
    })
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Best-effort writability probe: try to create (and remove) a temp file.
fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".anotherword-write-test");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Platform data directory, e.g. `~/.local/share/anotherword` (honors XDG on Linux).
pub fn data_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "anotherword")
        .context("could not determine an OS data directory")?;
    Ok(dirs.data_dir().to_path_buf())
}

/// Open a connection to an existing DB (does not create the schema).
pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("opening database at {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    conn.pragma_update(None, "foreign_keys", "ON").ok();
    Ok(conn)
}

/// Create the database file (and parent dir) and initialize the schema.
pub fn create(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    let conn = open(path)?;
    init_schema(&conn)?;
    Ok(conn)
}

/// True if the DB exists and carries a schema version we understand.
pub fn is_usable(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let Ok(conn) = open(path) else {
        return false;
    };
    let v: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='schema_version'", [], |r| {
            r.get(0)
        })
        .ok();
    matches!(v.and_then(|s| s.parse::<i64>().ok()), Some(v) if v == SCHEMA_VERSION)
}

/// Create all tables and indexes (idempotent).
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS languages (
            code TEXT PRIMARY KEY,
            name TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS words (
            id        INTEGER PRIMARY KEY,
            lang      TEXT NOT NULL REFERENCES languages(code),
            word      TEXT NOT NULL,
            word_key  TEXT NOT NULL,
            metaphone TEXT,
            UNIQUE(lang, word_key, word)
        );

        -- One row per (word, sense): this word's view of one synset.
        CREATE TABLE IF NOT EXISTS senses (
            id        INTEGER PRIMARY KEY,
            word_id   INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
            pos       TEXT NOT NULL,
            sense_num INTEGER NOT NULL,
            hint      TEXT,
            synonyms  TEXT,
            antonyms  TEXT,
            related   TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_senses_word ON senses(word_id);

        -- Irregular inflections from WordNet *.exc: 'mice' -> 'mouse'.
        CREATE TABLE IF NOT EXISTS exceptions (
            lang TEXT NOT NULL,
            pos  TEXT NOT NULL,
            form TEXT NOT NULL,
            base TEXT NOT NULL,
            PRIMARY KEY (lang, pos, form, base)
        );

        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT
        );
        "#,
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', ?1)",
        [SCHEMA_VERSION.to_string()],
    )?;
    Ok(())
}

/// Normalize a word for lookup: trimmed, lowercased, `_` and runs of whitespace
/// collapsed to a single space.
pub fn word_key(word: &str) -> String {
    word.replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::word_key;

    #[test]
    fn normalizes_keys() {
        assert_eq!(word_key("  Ice_Cream "), "ice cream");
        assert_eq!(word_key("ice   cream"), "ice cream");
    }
}
