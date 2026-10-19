# anotherword — Design

`anotherword` is a fast, fully offline command-line thesaurus. Give it a word and
it prints synonyms and antonyms, grouped by sense. No definitions.

```
$ anotherword large
large  (adj)
  1  [size]
     syn: big
     ant: small
  4
     syn: big, magnanimous
  ...

$ anotherword larg
No entry for "larg". Did you mean: large · larch · lard · largo · lark
```

It is a sibling of [`whatword`](../whatword/DESIGN.md) and deliberately reuses its
architecture: WordNet downloaded once, compiled into a portable SQLite file, then
every query is offline and instant.

---

## 1. Goals & non-goals

**Goals**

- Instant (<50 ms) offline lookup of synonyms and antonyms for an English word.
- Results grouped by part of speech and sense, so unrelated meanings of a word
  (*bank* the shore vs. *bank* the institution) are not blended together.
- Accept inflected input (`larger`, `mice`, `running`) by reducing it to a base form.
- On a miss, suggest similarly spelled real words ("did you mean").
- Optional related words (hypernyms, "see also") via `--related`.
- Portable: the DB lives beside the executable, as in whatword.
- Machine-readable output (`--json`).

**Non-goals (v1)**

- Definitions, pronunciation, etymology, usage examples.
- Online lookups or auto-updating data.
- Multiple words per invocation (exactly one word per run).
- Languages other than English (the schema is `lang`-keyed so it can come later).
- Antonyms beyond what WordNet records (see §2).

---

## 2. Data source

| Data | Source | License |
|------|--------|---------|
| Synonyms, antonyms, relations | **Princeton WordNet 3.1** | WordNet License (permissive) |

WordNet organizes words into **synsets**: sets of words that share one meaning.
The synonyms of a word *in a given sense* are the other members of its synset.
The same download as whatword is used
(`https://wordnetcode.princeton.edu/wn3.1.dict.tar.gz`) and cached in the DB
directory so `--build --force` works offline afterward.

### What we read

| WordNet file / pointer | Used for |
|------------------------|----------|
| `data.{noun,verb,adj,adv}` synset lines | synset members (synonyms), part of speech |
| `index.*` | sense ordering per lemma (frequency-ranked) |
| `!` antonym pointer | antonyms |
| `@` hypernym pointer | sense hint for nouns and verbs; `--related` |
| `=` attribute pointer | sense hint for adjectives (`large` → *size*) |
| `^` see-also, `&` similar-to | `--related` |
| `*.exc` exception lists | irregular inflections (`mice`→`mouse`, `better`→`good`) |

### Antonyms are sparse (important)

WordNet's antonym pointer is **lexical**: it links a specific *word* in a synset to
a specific *word* in another synset, not the synsets as a whole. Antonyms exist
mostly for adjectives, adverbs, and some verbs and nouns; plenty of words have none.
v1 shows only these **direct** antonyms. For adjectives, "satellite" synsets get no
direct antonym (`huge` has none, though its head word `large` does). The
head-via-similar-to inheritance that would fix this is deferred to §10.

Consequence for the CLI: a word that has synonyms but no antonyms is still a hit
(exit 0); the antonym line is simply omitted for that sense.

### Ordering and noise

- Senses use WordNet's own frequency-based `index.*` order, as in whatword.
- Within a sense, synonyms are listed in synset order (WordNet's own).
- The queried word is excluded from its own synonym list.
- Multi-word lemmas (`ice_cream`) are shown with spaces; underscores are
  normalized away at build time.
- Adjective markers such as `(a)`/`(p)` suffixes in `data.adj` are stripped.
- Proper-noun instance synsets (*Paris*) are kept only because they appear as
  lemmas, but a headword is only listed if it has a synonym, antonym, or related
  word to show. Empty entries are dropped at build time.

---

## 3. Storage: SQLite schema

A single file, `anotherword.sqlite`, with every table keyed by `lang` (`'en'`).

```sql
CREATE TABLE languages (code TEXT PRIMARY KEY, name TEXT NOT NULL);

CREATE TABLE words (
    id        INTEGER PRIMARY KEY,
    lang      TEXT NOT NULL REFERENCES languages(code),
    word      TEXT NOT NULL,        -- display form: 'large'
    word_key  TEXT NOT NULL,        -- lowercased, trimmed, '_' -> ' '
    metaphone TEXT,                 -- suggestion key
    UNIQUE(lang, word_key, word)
);
CREATE INDEX idx_words_meta ON words(metaphone);

-- One row per (word, sense): this word's view of one synset.
CREATE TABLE senses (
    id         INTEGER PRIMARY KEY,
    word_id    INTEGER NOT NULL REFERENCES words(id) ON DELETE CASCADE,
    pos        TEXT NOT NULL,       -- 'n','v','adj','adv'
    sense_num  INTEGER NOT NULL,    -- 1 = most common
    hint       TEXT,                -- short disambiguating label, nullable
    synonyms   TEXT,                -- 'big, great, grand', nullable
    antonyms   TEXT,                -- 'small', nullable
    related    TEXT                 -- 'hypernym:...|seealso:...', nullable (--related)
);
CREATE INDEX idx_senses_word ON senses(word_id);

-- Inflection exceptions from WordNet *.exc: 'mice' -> 'mouse'.
CREATE TABLE exceptions (
    lang TEXT NOT NULL,
    pos  TEXT NOT NULL,
    form TEXT NOT NULL,
    base TEXT NOT NULL,
    PRIMARY KEY (lang, pos, form, base)
);

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);  -- schema version, build date
```

Synonym and antonym text is short and a thesaurus DB is far smaller than
whatword's (no glosses), so v1 stores it as **plain text**, with no zstd dictionary.
`textcodec` can be added later behind the same accessors if size matters.

`antonyms` is stored per sense and is **per word**: it reflects the pointer for *this*
lemma in the synset, because WordNet antonyms are lexical.

### Sense hints

Synonym lists alone don't say which meaning is which, and we have no definitions, so
each sense gets a short hint:

- **noun / verb:** the first hypernym's first lemma (`bank` → *depository financial
  institution*, *sloping land*).
- **adjective:** the attribute noun if there is one (`large` → *size*); otherwise
  none.
- **adverb:** none.

Hints are best-effort and omitted when unavailable; the sense number is always shown.

### DB location (identical to whatword)

1. `ANOTHERWORD_DB` env var, if set.
2. `<dir-of-exe>/anotherword.sqlite` (portable default; used if it exists or, for
   `--build`, if the directory is writable).
3. OS data dir via the `directories` crate (`~/.local/share/anotherword/`,
   `~/Library/Application Support/anotherword/`, `%APPDATA%\anotherword\`).

`--where` prints the winner. Cached source archives live in the same directory.

---

## 4. Lookup pipeline

```
input → normalize → exact match? ─yes→ render
                       │ no
                       ▼
                 lemmatize (exceptions, then suffix rules)
                       │ candidate base forms, each checked against `words`
                       ▼
                 found? ─yes→ render (note: "showing results for 'large'")
                       │ no
                       ▼
                 suggest → "No entry for …. Did you mean: …" (exit 1)
```

- **Normalize:** trim, lowercase, collapse internal whitespace to single spaces.
  `ice cream` and `Ice_Cream` both find `ice cream`.
- **Lemmatize** follows WordNet's `morphy` algorithm. First, check the `exceptions`
  table. Then try detachment rules in order, keeping candidates that exist in
  `words`:
  - nouns: `s→""`, `ses→s`, `xes→x`, `zes→z`, `ches→ch`, `shes→sh`, `men→man`, `ies→y`
  - verbs: `s→""`, `ies→y`, `es→e`, `es→""`, `ed→e`, `ed→""`, `ing→e`, `ing→""`
  - adjectives: `er→""`, `er→e`, `est→""`, `est→e`
- An exact match always wins over a lemma (`left` is its own word as well as a form
  of `leave`). If an inflected form maps to several bases (`saw` → *saw*, *see*), all
  matching bases are shown, grouped by base.
- When the result came from a base form, the output says so on one line so the user
  isn't confused about why the header differs from their input.

### Suggestions

Same two stages as whatword's `suggest.rs`, ported with minimal change:

1. **Coarse filter (SQL):** words sharing the Metaphone code, plus a length-window
   and first-letter bucket.
2. **Rank (Rust):** Damerau-Levenshtein distance (`strsim`) with a small bonus for a
   matching first letter and similar length. Default top 8, `--max-suggestions N`.

Suggestions are shown on a miss only (unlike whatword, there's no "similar words on
a hit" section, since the point here is the synonym list itself).

---

## 5. CLI surface

Following whatword's rule: **no bare subcommands.** The only positional argument is
the word; every operation is a `--` flag, so `anotherword build` looks up *build*.

```
anotherword <word>              Look up synonyms/antonyms (exactly one word)

Lookup modifiers:
  --related                     Also show related words (hypernyms, see-also)
  --max <N>                     Cap words per synonym/antonym list (default: unlimited)
  --lang <code>                 Language (default: en)
  --width <N>                   Hard-wrap output to N columns (default: no wrapping)
  --json                        Emit structured JSON
  --no-color                    Plain text (color is the default on a terminal)

Output-section toggles:
  --no-synonyms                 Suppress synonym lines
  --no-antonyms                 Suppress antonym lines
  --max-suggestions <N>         Did-you-mean count on a miss (default: 8)

Operations (mutually exclusive with a lookup):
  --build [--force]             (Re)build the database from WordNet
  --where                       Print the resolved database path
  --count                       Print word/sense counts and the absolute DB path

  --version   --help
```

Parsed with `clap` (derive). Exactly one positional word or exactly one operation.

**First run:** if the DB is missing when a lookup runs, print a one-line notice,
build it automatically, then answer the query.

### Output format

- Plain greppable text; no wrapping unless `--width` is given.
- Per sense: `<n>  <hint>` then indented `syn:` and `ant:` lines. With
  `--related`, a `rel:` line follows. Senses with nothing left to show after the
  `--no-*` toggles are skipped.
- Senses are grouped under a part-of-speech header: `large  (adj)`, `(n)`, `(v)`,
  `(adv)`.
- Color (bold headword, dim hints, `ant:` lines in a distinct color) is on by
  default for a TTY, off for pipes, when `NO_COLOR` is set, or with `--no-color`.
- `--json` returns `{ "query", "found": true, "lemmatized", "entries": [ { "word",
  "senses": [ { "pos", "num", "hint", "synonyms", "antonyms", "related" } ] } ] }`,
  omitting empty or suppressed keys. On a miss: `{ "query", "found": false,
  "suggestions": [...] }`.
- Sense numbers are WordNet's, so they can have gaps: senses with nothing to show
  are skipped but keep their original number.
- An exact entry wins over a base form, so `larger` shows WordNet's own `larger`
  entry, not `large`.

### Exit codes (grep convention, same as whatword)

| code | meaning |
|------|---------|
| `0` | word found (even if no antonyms exist) |
| `1` | word not found (suggestions still print) |
| `2` | usage error |

If all sections are suppressed, nothing prints but the exit code still reports
found/not found, which makes `anotherword --no-synonyms --no-antonyms word` a quiet
membership test.

---

## 6. Build pipeline

`anotherword --build`:

1. **Download** (`build/download.rs`): fetch the WordNet tarball, cache it. Skipped if cached.
2. **Parse** (`build/wordnet.rs`):
   - Pass 1: read every `data.*` synset line into memory: offset, POS, member
     lemmas (with lexical ids), and pointers `(symbol, target offset, target POS,
     source/target word numbers)`.
   - Pass 2: for each synset and each member lemma, emit one `senses` row:
     synonyms = other members; antonyms = targets of `!` pointers whose source
     word number is this lemma; hint and related from `@`, `=`, `^`, `&` pointers.
   - Sense order comes from `index.*` (lemma → synset offsets, in frequency order).
3. **Exceptions:** load `*.exc` into the `exceptions` table.
4. **Metaphone:** compute for every headword (`rphonetic`).
5. **Finalize:** drop rows with nothing to show, build indexes, `VACUUM`, write `meta`.

Building takes seconds and runs inside one transaction.

---

## 7. Extensibility

Same shape as whatword; the query path never knows where data came from.

```
CLI (clap) → Thesaurus trait → SqliteThesaurus
                lookup(word, lang) -> Option<Lookup>
                suggest(word, lang, n) -> Vec<Suggestion>

Build side: SourceImporter trait → WordNetImporter
            (future: MobyImporter, WiktionaryImporter, an es/fr importer)
```

- New **language** or **source** = a new `SourceImporter` writing `senses` rows with
  another `lang`. A second source (e.g., Moby) could simply append synonyms to the
  same rows or to a new sense group; `senses.synonyms` stays opaque to the renderer.
- Lemmatization rules are per-language, behind a small `Lemmatizer` trait
  (`english::Morphy` in v1).

### Module layout

```
anotherword/
├── Cargo.toml
├── DESIGN.md
├── README.md
└── src/
    ├── main.rs             # dispatch, exit codes
    ├── cli.rs              # clap definitions
    ├── db.rs               # open/create, schema, DbWriter, path resolution
    ├── thesaurus.rs        # Thesaurus trait + SqliteThesaurus
    ├── lemma.rs            # Lemmatizer trait + English morphy
    ├── suggest.rs          # metaphone filter + edit-distance rank
    ├── output.rs           # text, color, wrapping, JSON rendering
    ├── model.rs            # Lookup, Sense, Suggestion structs
    └── build/
        ├── mod.rs          # orchestration
        ├── download.rs     # fetch + cache
        └── wordnet.rs      # WordNetImporter (SourceImporter)
```

`db.rs`, `suggest.rs`, `build/download.rs` are lifted from whatword and renamed.

### Dependencies

`clap`, `rusqlite` (bundled), `strsim`, `rphonetic`, `serde`/`serde_json`,
`ureq` (build only), `flate2` + `tar`, `directories`, `textwrap`, `anyhow`.
Dropped relative to whatword: `csv` (no export), `zstd` (no compression).

---

## 8. Edge cases

- **No DB, no network:** clear message pointing to `anotherword --build` or
  `ANOTHERWORD_DB`.
- **Empty or whitespace input / unknown `--lang`:** friendly error, exit 2.
- **Word with synonyms but no antonyms:** antonym line omitted, exit 0.
- **Word whose only entries are empty after suppression flags:** no output, exit 0.
- **Hyphens and apostrophes:** preserved in `word_key`; looked up exactly.
- **Very long synonym lists:** unwrapped by default (terminal soft-wraps);
  `--max N` truncates each list; `--width N` hard-wraps.
- **Stdout closed early (`| head`):** broken pipe exits quietly.

---

## 9. Testing

- Unit tests: lemmatizer golden cases (`mice`, `larger`, `running`, `better`,
  `ponies`, `boxes`), sense-hint selection, suggestion ranking.
- WordNet parser tests against a small fixture of real `data.*`/`index.*` lines,
  including a lexical-antonym case (word-number mapping) and a satellite adjective.
- Integration test: build a tiny fixture DB and run `anotherword large`,
  `--json`, `--no-antonyms`, a miss, and an inflected form, checking output and
  exit codes.

---

## 10. Open questions / future work

- **Indirect antonyms** for adjective satellites via similar-to to the head
  adjective (`huge` → `small`). Better coverage, some noise; could be a flag.
- **Moby Thesaurus** (public domain) as a second importer for broader, sense-less
  synonym coverage.
- **Ranking** synonyms by corpus frequency (WordNet tag counts) instead of synset order.
- Multiple words per invocation; shell completions (`clap_complete`); a prebuilt-DB
  release artifact for zero-network installs.
