//! WordNet 3.1 importer.
//!
//! Reads the `data.*` files (synsets: member words + typed pointers) and the
//! `index.*` files (each lemma's synsets in sense-number order, i.e. most common
//! first). For every (lemma, synset) pair it emits one sense row holding the
//! other synset members (synonyms), the lemma's *lexical* antonyms, a short hint,
//! and related words. Also loads the `*.exc` irregular-inflection lists.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};

use super::download;
use super::{DbWriter, SourceImporter};
use crate::model::Related;

pub struct WordNetImporter {
    lang: String,
}

impl WordNetImporter {
    pub fn new(lang: &str) -> Self {
        Self {
            lang: lang.to_string(),
        }
    }
}

/// A synset is keyed by (file part-of-speech letter, byte offset string).
type SynsetKey = (char, String);

/// One pointer from a synset to another.
#[derive(Debug)]
struct Ptr {
    symbol: String,
    target: SynsetKey,
    /// 1-based member of *this* synset the pointer is from; 0 = whole synset.
    src: usize,
    /// 1-based member of the *target* synset; 0 = whole synset.
    tgt: usize,
}

#[derive(Debug)]
struct Synset {
    /// Display part of speech: `n`, `v`, `adj`, `adv`.
    pos: &'static str,
    /// Member words, `_` turned into spaces and markers stripped.
    words: Vec<String>,
    ptrs: Vec<Ptr>,
}

impl SourceImporter for WordNetImporter {
    fn name(&self) -> &str {
        "WordNet 3.1 (thesaurus)"
    }

    fn import(&self, writer: &mut DbWriter, cache_dir: &Path) -> Result<()> {
        let archive = cache_dir.join("wn3.1.dict.tar.gz");
        let extract_dir = cache_dir.join("wordnet");
        // The tarball unpacks to a top-level `dict/` directory.
        let dict_dir = extract_dir.join("dict");
        download::fetch_and_extract_targz(download::WORDNET_URL, &archive, &extract_dir, &dict_dir)?;

        // file pos letter -> (display pos, data file, index file, exception file)
        let parts = [
            ('n', "n", "data.noun", "index.noun", "noun.exc"),
            ('v', "v", "data.verb", "index.verb", "verb.exc"),
            ('a', "adj", "data.adj", "index.adj", "adj.exc"),
            ('r', "adv", "data.adv", "index.adv", "adv.exc"),
        ];

        let mut synsets: HashMap<SynsetKey, Synset> = HashMap::new();
        for (fp, _, data_file, _, _) in parts {
            let path = dict_dir.join(data_file);
            load_data_file(&path, fp, &mut synsets)
                .with_context(|| format!("reading {}", path.display()))?;
        }

        for (fp, _, _, index_file, _) in parts {
            let path = dict_dir.join(index_file);
            self.import_index_file(&path, fp, &synsets, writer)
                .with_context(|| format!("reading {}", path.display()))?;
        }

        for (_, pos, _, _, exc_file) in parts {
            let path = dict_dir.join(exc_file);
            self.import_exceptions(&path, pos, writer)
                .with_context(|| format!("reading {}", path.display()))?;
        }
        Ok(())
    }
}

impl WordNetImporter {
    fn import_index_file(
        &self,
        path: &Path,
        fpos: char,
        synsets: &HashMap<SynsetKey, Synset>,
        writer: &mut DbWriter,
    ) -> Result<()> {
        let file = std::fs::File::open(path)?;
        for line in BufReader::new(file).lines() {
            let line = line?;
            let Some((lemma, offsets)) = parse_index_line(&line) else {
                continue;
            };
            let display = lemma.replace('_', " ");
            for (i, offset) in offsets.iter().enumerate() {
                let Some(syn) = synsets.get(&(fpos, offset.clone())) else {
                    continue;
                };
                let built = build_sense(syn, &display, synsets);
                writer.add_sense(
                    &self.lang,
                    &display,
                    syn.pos,
                    (i + 1) as i64,
                    built.hint.as_deref(),
                    &built.synonyms,
                    &built.antonyms,
                    &built.related,
                )?;
            }
        }
        Ok(())
    }

    fn import_exceptions(&self, path: &Path, pos: &str, writer: &mut DbWriter) -> Result<()> {
        let file = std::fs::File::open(path)?;
        for line in BufReader::new(file).lines() {
            let line = line?;
            let mut it = line.split_whitespace();
            let Some(form) = it.next() else { continue };
            for base in it {
                writer.add_exception(
                    &self.lang,
                    pos,
                    &form.replace('_', " "),
                    &base.replace('_', " "),
                )?;
            }
        }
        Ok(())
    }
}

/// What one (lemma, synset) pair contributes.
#[derive(Debug, Default)]
struct BuiltSense {
    hint: Option<String>,
    synonyms: Vec<String>,
    antonyms: Vec<String>,
    related: Vec<Related>,
}

fn build_sense(syn: &Synset, lemma: &str, all: &HashMap<SynsetKey, Synset>) -> BuiltSense {
    let is_self = |w: &str| w.eq_ignore_ascii_case(lemma);
    let mut out = BuiltSense {
        synonyms: syn.words.iter().filter(|w| !is_self(w)).cloned().collect(),
        ..Default::default()
    };

    // Antonym pointers are lexical: only those whose source is *this* lemma count.
    let member = syn.words.iter().position(|w| is_self(w)).map(|i| i + 1);
    if let Some(member) = member {
        for p in syn.ptrs.iter().filter(|p| p.symbol == "!" && p.src == member) {
            if let Some(target) = all.get(&p.target) {
                if let Some(word) = target.words.get(p.tgt.saturating_sub(1)) {
                    push_unique(&mut out.antonyms, word.clone());
                }
            }
        }
    }

    // Hint: first hypernym (nouns/verbs) or attribute noun (adjectives).
    let hint_symbols: &[&str] = match syn.pos {
        "n" | "v" => &["@", "@i"],
        "adj" => &["="],
        _ => &[],
    };
    out.hint = syn
        .ptrs
        .iter()
        .filter(|p| hint_symbols.contains(&p.symbol.as_str()))
        .find_map(|p| all.get(&p.target))
        .and_then(|t| t.words.first().cloned());

    // Related words, one group per kind.
    for (kind, symbols) in [
        ("hypernym", &["@", "@i"][..]),
        ("similar", &["&"][..]),
        ("see also", &["^"][..]),
    ] {
        let mut words: Vec<String> = Vec::new();
        for p in syn.ptrs.iter().filter(|p| symbols.contains(&p.symbol.as_str())) {
            let Some(target) = all.get(&p.target) else {
                continue;
            };
            if p.tgt > 0 {
                if let Some(w) = target.words.get(p.tgt - 1) {
                    push_unique(&mut words, w.clone());
                }
            } else {
                for w in &target.words {
                    push_unique(&mut words, w.clone());
                }
            }
        }
        words.retain(|w| !is_self(w));
        if !words.is_empty() {
            out.related.push(Related {
                kind: kind.to_string(),
                words,
            });
        }
    }
    out
}

fn push_unique(list: &mut Vec<String>, word: String) {
    if !list.contains(&word) {
        list.push(word);
    }
}

fn load_data_file(path: &Path, fpos: char, out: &mut HashMap<SynsetKey, Synset>) -> Result<()> {
    let file = std::fs::File::open(path)?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        // License/header lines begin with a space.
        if line.starts_with(' ') {
            continue;
        }
        if let Some((offset, synset)) = parse_data_line(&line) {
            out.insert((fpos, offset), synset);
        }
    }
    Ok(())
}

/// Parse one `data.*` synset line. Returns (offset, synset).
///
/// Layout: `offset lex_filenum ss_type w_cnt {word lex_id}* p_cnt {ptr}* ... | gloss`
/// where each ptr is `symbol offset pos src_tgt` and `src_tgt` is two hex bytes.
fn parse_data_line(line: &str) -> Option<(String, Synset)> {
    let (pre, _gloss) = line.split_once('|')?;
    let mut it = pre.split_whitespace();
    let offset = it.next()?.to_string();
    let _lex_filenum = it.next()?;
    let ss_type = it.next()?;
    let w_cnt = usize::from_str_radix(it.next()?, 16).ok()?;

    let mut words = Vec::with_capacity(w_cnt);
    for _ in 0..w_cnt {
        words.push(clean_word(it.next()?));
        let _lex_id = it.next()?;
    }

    let p_cnt: usize = it.next()?.parse().ok()?;
    let mut ptrs = Vec::with_capacity(p_cnt);
    for _ in 0..p_cnt {
        let symbol = it.next()?.to_string();
        let target_offset = it.next()?.to_string();
        let target_pos = it.next()?.chars().next()?;
        let st = it.next()?;
        if st.len() != 4 {
            return None;
        }
        ptrs.push(Ptr {
            symbol,
            // Satellite adjectives are referenced as 'a' pointers into data.adj.
            target: (if target_pos == 's' { 'a' } else { target_pos }, target_offset),
            src: usize::from_str_radix(&st[..2], 16).ok()?,
            tgt: usize::from_str_radix(&st[2..], 16).ok()?,
        });
    }

    Some((
        offset,
        Synset {
            pos: pos_from_ss_type(ss_type),
            words,
            ptrs,
        },
    ))
}

/// Parse one `index.*` line into (lemma, ordered synset offsets).
fn parse_index_line(line: &str) -> Option<(String, Vec<String>)> {
    if line.starts_with(' ') {
        return None;
    }
    let mut it = line.split_whitespace();
    let lemma = it.next()?.to_string();
    let _pos = it.next()?;
    let synset_cnt: usize = it.next()?.parse().ok()?;
    let p_cnt: usize = it.next()?.parse().ok()?;
    for _ in 0..p_cnt {
        it.next()?; // pointer symbols
    }
    let _sense_cnt = it.next()?;
    let _tagsense_cnt = it.next()?;
    let offsets: Vec<String> = (0..synset_cnt)
        .map(|_| it.next().map(str::to_string))
        .collect::<Option<_>>()?;
    Some((lemma, offsets))
}

fn pos_from_ss_type(ss_type: &str) -> &'static str {
    match ss_type {
        "n" => "n",
        "v" => "v",
        "a" | "s" => "adj",
        "r" => "adv",
        _ => "n",
    }
}

/// WordNet words use `_` for spaces and may carry syntactic markers like
/// `word(a)`. Normalize to a display form.
fn clean_word(word: &str) -> String {
    let base = match word.find('(') {
        Some(i) => &word[..i],
        None => word,
    };
    base.replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic but format-correct lines: `large` (member 1) has the lexical
    // antonym `small`; `big` (member 2) has none.
    const LARGE: &str = "00001740 00 a 02 large 0 big 0 003 ! 00002000 a 0101 & 00003000 s 0000 = 05000000 n 0000 | above average size";
    const SMALL: &str = "00002000 00 a 01 small 0 001 ! 00001740 a 0101 | below average size";
    const SIZE: &str = "05000000 04 n 01 size 0 000 | the magnitude of something";
    const HUGE: &str = "00003000 00 s 02 huge 0 enormous(a) 0 000 | extremely large";

    fn world() -> HashMap<SynsetKey, Synset> {
        let mut m = HashMap::new();
        for (fpos, line) in [('a', LARGE), ('a', SMALL), ('n', SIZE), ('a', HUGE)] {
            let (off, syn) = parse_data_line(line).unwrap();
            m.insert((fpos, off), syn);
        }
        m
    }

    #[test]
    fn parses_words_and_pointers() {
        let (offset, syn) = parse_data_line(LARGE).unwrap();
        assert_eq!(offset, "00001740");
        assert_eq!(syn.pos, "adj");
        assert_eq!(syn.words, vec!["large", "big"]);
        assert_eq!(syn.ptrs.len(), 3);
        assert_eq!((syn.ptrs[0].src, syn.ptrs[0].tgt), (1, 1));
        // Satellite pointer targets normalize to the 'a' file.
        assert_eq!(syn.ptrs[1].target, ('a', "00003000".to_string()));
    }

    #[test]
    fn lexical_antonym_applies_only_to_its_own_lemma() {
        let w = world();
        let syn = &w[&('a', "00001740".to_string())];
        let large = build_sense(syn, "large", &w);
        assert_eq!(large.antonyms, vec!["small"]);
        assert_eq!(large.synonyms, vec!["big"]);
        let big = build_sense(syn, "big", &w);
        assert!(big.antonyms.is_empty());
        assert_eq!(big.synonyms, vec!["large"]);
    }

    #[test]
    fn adjective_hint_is_attribute_noun_and_related_is_grouped() {
        let w = world();
        let syn = &w[&('a', "00001740".to_string())];
        let large = build_sense(syn, "large", &w);
        assert_eq!(large.hint.as_deref(), Some("size"));
        assert_eq!(large.related.len(), 1);
        assert_eq!(large.related[0].kind, "similar");
        assert_eq!(large.related[0].words, vec!["huge", "enormous"]);
    }

    #[test]
    fn index_line_offsets_in_order() {
        let line = "large adj 2 2 & ! 2 1 00001740 00009999";
        let (lemma, offsets) = parse_index_line(line).unwrap();
        assert_eq!(lemma, "large");
        assert_eq!(offsets, vec!["00001740", "00009999"]);
    }

    #[test]
    fn strips_markers_and_underscores() {
        assert_eq!(clean_word("beautiful(a)"), "beautiful");
        assert_eq!(clean_word("polar_bear"), "polar bear");
    }
}
