//! anotherword: a fast, offline command-line thesaurus.

mod build;
mod cli;
mod db;
mod lemma;
mod model;
mod output;
mod suggest;
mod thesaurus;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Result};
use clap::Parser;

use cli::Cli;
use model::Lookup;
use output::OutputOptions;
use thesaurus::{SqliteThesaurus, Thesaurus};

/// Exit codes (grep-style): 0 found, 1 not found, 2 usage/other error.
const EXIT_FOUND: u8 = 0;
const EXIT_NOT_FOUND: u8 = 1;
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) if is_broken_pipe(&e) => ExitCode::from(EXIT_FOUND),
        Err(e) => {
            eprintln!("anotherword: error: {e:#}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn is_broken_pipe(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<std::io::Error>())
        .any(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
}

fn run(cli: Cli) -> Result<u8> {
    if cli.operation_count() > 1 {
        bail!("--build, --where, and --count cannot be combined");
    }
    if cli.operation_count() == 1 && cli.word.is_some() {
        bail!("cannot look up a word and run an operation at the same time");
    }

    let location = db::resolve_location()?;

    if cli.r#where {
        println!("{}", absolute_path(&location.path).display());
        eprintln!("({})", location.source.describe());
        return Ok(EXIT_FOUND);
    }

    if cli.build {
        build::build_database(&location.path, cli.force)?;
        return Ok(EXIT_FOUND);
    }

    if cli.count {
        return run_count(&location.path);
    }

    match &cli.word {
        Some(word) => run_lookup(&location.path, &cli, word),
        None => {
            use clap::CommandFactory;
            Cli::command().print_help().ok();
            println!();
            Ok(EXIT_USAGE)
        }
    }
}

/// Open the thesaurus, auto-building on first use if the DB is missing.
fn open_thesaurus(db_path: &Path) -> Result<SqliteThesaurus> {
    if !db::is_usable(db_path) {
        eprintln!("anotherword: no thesaurus database found; building it now (one-time setup).");
        if let Err(e) = build::build_database(db_path, true) {
            return Err(e.context(
                "failed to build the thesaurus database. Run `anotherword --build` while \
                 online, or set ANOTHERWORD_DB to a prebuilt database.",
            ));
        }
    }
    Ok(SqliteThesaurus::new(db::open(db_path)?))
}

fn run_lookup(db_path: &Path, cli: &Cli, word: &str) -> Result<u8> {
    if word.trim().is_empty() {
        bail!("no word given");
    }
    let thes = open_thesaurus(db_path)?;
    if !thes.has_language(&cli.lang) {
        bail!("unknown language \"{}\"", cli.lang);
    }
    let opts = OutputOptions {
        width: cli.width,
        color: !cli.no_color && OutputOptions::detect_color(),
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let stderr = std::io::stderr();
    let mut err = stderr.lock();

    match thes.lookup(word, &cli.lang)? {
        Some(mut lookup) => {
            shape(&mut lookup, cli);
            if cli.json {
                writeln!(out, "{}", hit_json(&lookup))?;
            } else {
                output::render_hit(&mut out, &mut err, &lookup, &opts)?;
            }
            Ok(EXIT_FOUND)
        }
        None => {
            let suggestions = thes.suggest(word, &cli.lang, cli.max_suggestions)?;
            if cli.json {
                let words: Vec<&String> = suggestions.iter().map(|s| &s.word).collect();
                let json = serde_json::json!({
                    "query": word,
                    "found": false,
                    "suggestions": words,
                });
                writeln!(out, "{json}")?;
            } else {
                output::render_miss(&mut out, &mut err, word, &suggestions, &opts)?;
            }
            Ok(EXIT_NOT_FOUND)
        }
    }
}

/// Apply the section toggles and `--max` to a lookup before rendering.
fn shape(lookup: &mut Lookup, cli: &Cli) {
    for sense in lookup.entries.iter_mut().flat_map(|e| e.senses.iter_mut()) {
        if cli.no_synonyms {
            sense.synonyms.clear();
        }
        if cli.no_antonyms {
            sense.antonyms.clear();
        }
        if !cli.related {
            sense.related.clear();
        }
        if let Some(max) = cli.max {
            sense.synonyms.truncate(max);
            sense.antonyms.truncate(max);
            for rel in &mut sense.related {
                rel.words.truncate(max);
            }
        }
    }
    lookup.prune();
}

fn hit_json(lookup: &Lookup) -> String {
    let mut value = serde_json::to_value(lookup).unwrap_or_default();
    value["found"] = serde_json::json!(true);
    value.to_string()
}

fn run_count(db_path: &Path) -> Result<u8> {
    let thes = open_thesaurus(db_path)?;
    let conn = thes.connection();
    let words: i64 = conn.query_row("SELECT count(*) FROM words", [], |r| r.get(0))?;
    let senses: i64 = conn.query_row("SELECT count(*) FROM senses", [], |r| r.get(0))?;
    let with_ant: i64 = conn.query_row(
        "SELECT count(DISTINCT word_id) FROM senses WHERE antonyms IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    println!("words:         {words}");
    println!("senses:        {senses}");
    println!("with antonyms: {with_ant}");
    println!("database:      {}", absolute_path(db_path).display());
    Ok(EXIT_FOUND)
}

/// Best-effort absolute path: canonicalize if the file exists (resolves symlinks),
/// otherwise fall back to a lexically-absolute form.
fn absolute_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}
