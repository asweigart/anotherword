//! Rendering of lookup results to the terminal.
//!
//! The renderer prints whatever the `Lookup` still contains; `--no-*` toggles and
//! `--max` are applied to the `Lookup` beforehand (see `main::shape`), so
//! suppressing every section yields no output at all.

use std::io::{IsTerminal, Write};

use crate::model::{Lookup, Sense, Suggestion};

pub struct OutputOptions {
    pub width: Option<usize>,
    pub color: bool,
}

impl OutputOptions {
    /// Color on only when stdout is a TTY and NO_COLOR is unset.
    pub fn detect_color() -> bool {
        std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
    }
}

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

fn styled(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("{code}{text}{RESET}")
    } else {
        text.to_string()
    }
}

/// Render a found lookup: per headword, per part of speech, per sense.
pub fn render_hit(
    out: &mut dyn Write,
    err: &mut dyn Write,
    lookup: &Lookup,
    opts: &OutputOptions,
) -> std::io::Result<()> {
    if lookup.lemmatized && !lookup.entries.is_empty() {
        let bases: Vec<String> = lookup.entries.iter().map(|e| format!("\"{}\"", e.word)).collect();
        writeln!(
            err,
            "Showing results for {} (from \"{}\").",
            bases.join(", "),
            lookup.query
        )?;
    }

    for (n, entry) in lookup.entries.iter().enumerate() {
        if n > 0 {
            writeln!(out)?;
        }
        let mut current_pos: Option<&str> = None;
        for sense in &entry.senses {
            if current_pos != Some(sense.pos.as_str()) {
                current_pos = Some(sense.pos.as_str());
                let header = format!(
                    "{}  {}",
                    styled(&entry.word, BOLD, opts.color),
                    styled(&format!("({})", sense.pos), DIM, opts.color)
                );
                writeln!(out, "{header}")?;
            }
            render_sense(out, sense, opts)?;
        }
    }
    Ok(())
}

fn render_sense(out: &mut dyn Write, sense: &Sense, opts: &OutputOptions) -> std::io::Result<()> {
    let mut line = format!("  {}", sense.num);
    if let Some(hint) = &sense.hint {
        line.push_str("  ");
        line.push_str(&styled(&format!("[{hint}]"), DIM, opts.color));
    }
    writeln!(out, "{line}")?;

    if !sense.synonyms.is_empty() {
        write_list(out, "syn:", &sense.synonyms, None, opts)?;
    }
    if !sense.antonyms.is_empty() {
        write_list(out, "ant:", &sense.antonyms, Some(RED), opts)?;
    }
    for rel in &sense.related {
        write_list(out, &format!("{}:", rel.kind), &rel.words, Some(DIM), opts)?;
    }
    Ok(())
}

/// Write `label word, word, ...` indented under its sense, wrapping if requested.
fn write_list(
    out: &mut dyn Write,
    label: &str,
    words: &[String],
    style: Option<&str>,
    opts: &OutputOptions,
) -> std::io::Result<()> {
    let text = format!("{label} {}", words.join(", "));
    let indent = "     ";
    let body = match opts.width {
        Some(w) => {
            let options = textwrap::Options::new(w.max(20))
                .initial_indent(indent)
                .subsequent_indent("          ");
            textwrap::fill(&text, options)
        }
        None => format!("{indent}{text}"),
    };
    match style {
        Some(code) => writeln!(out, "{}", styled(&body, code, opts.color)),
        None => writeln!(out, "{body}"),
    }
}

/// Render a miss: a note on stderr and suggestions on stdout (pipe-capturable).
pub fn render_miss(
    out: &mut dyn Write,
    err: &mut dyn Write,
    word: &str,
    suggestions: &[Suggestion],
    opts: &OutputOptions,
) -> std::io::Result<()> {
    writeln!(err, "No entry for \"{word}\".")?;
    if !suggestions.is_empty() {
        let words: Vec<&str> = suggestions.iter().map(|s| s.word.as_str()).collect();
        writeln!(
            out,
            "{}: {}",
            styled("Did you mean", DIM, opts.color),
            words.join(" · ")
        )?;
    }
    Ok(())
}
