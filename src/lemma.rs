//! Inflection → base-form candidates, following WordNet's `morphy` detachment
//! rules. Irregular forms (`mice`, `better`) come from the `exceptions` table;
//! this module supplies only the regular suffix rules. Callers keep a candidate
//! only if it exists in the database.

/// (part of speech, suffix to strip, replacement)
const RULES: &[(&str, &str, &str)] = &[
    ("n", "s", ""),
    ("n", "ses", "s"),
    ("n", "xes", "x"),
    ("n", "zes", "z"),
    ("n", "ches", "ch"),
    ("n", "shes", "sh"),
    ("n", "men", "man"),
    ("n", "ies", "y"),
    ("v", "s", ""),
    ("v", "ies", "y"),
    ("v", "es", "e"),
    ("v", "es", ""),
    ("v", "ed", "e"),
    ("v", "ed", ""),
    ("v", "ing", "e"),
    ("v", "ing", ""),
    ("adj", "er", ""),
    ("adj", "er", "e"),
    ("adj", "est", ""),
    ("adj", "est", "e"),
];

/// Candidate `(pos, base)` pairs for `word`, in rule order, without duplicates.
pub fn candidates(word: &str) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for &(pos, suffix, repl) in RULES {
        if word.len() > suffix.len() && word.ends_with(suffix) {
            let base = format!("{}{}", &word[..word.len() - suffix.len()], repl);
            if !out.iter().any(|(p, b)| *p == pos && *b == base) {
                out.push((pos, base));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(word: &str, pos: &str, base: &str) -> bool {
        candidates(word).iter().any(|(p, b)| *p == pos && b == base)
    }

    #[test]
    fn regular_noun_plurals() {
        assert!(has("boxes", "n", "box"));
        assert!(has("ponies", "n", "pony"));
        assert!(has("churches", "n", "church"));
        assert!(has("women", "n", "woman"));
    }

    #[test]
    fn regular_verb_forms() {
        assert!(has("running", "v", "runn")); // later filtered: not a word
        assert!(has("making", "v", "make"));
        assert!(has("jumped", "v", "jump"));
        assert!(has("loved", "v", "love"));
    }

    #[test]
    fn adjective_degrees() {
        assert!(has("larger", "adj", "large"));
        assert!(has("biggest", "adj", "bigg")); // doubled consonants need the exceptions table
        assert!(has("taller", "adj", "tall"));
    }

    #[test]
    fn never_strips_a_whole_word() {
        assert!(candidates("s").is_empty());
    }
}
