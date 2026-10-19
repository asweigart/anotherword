# anotherword

A fast, fully **offline** command-line thesaurus. Give it a word and get synonyms
and antonyms, grouped by sense. No definitions.

```console
$ anotherword large
large  (adj)
  1  [size]
     syn: big
     ant: small
  ...
```

See [DESIGN.md](DESIGN.md) for the full design.

## Build

```console
$ cargo build --release
$ target/release/anotherword --build   # one-time: downloads WordNet 3.1 (~12 MB), builds the DB
```

The first lookup builds the database automatically if it is missing. After that
everything is offline.

## Usage

```
anotherword <word>

  --related            Also show hypernyms, similar-to, see-also
  --max <N>            Cap words per list
  --no-synonyms        Hide synonym lines
  --no-antonyms        Hide antonym lines
  --json               Structured output
  --width <N>          Hard-wrap at N columns
  --no-color           Plain text
  --lang <code>        Language (default: en)
  --max-suggestions N  Did-you-mean count on a miss (default: 8)

  --build [--force]    (Re)build the database
  --where              Print the database path
  --count              Print counts and the database path
```

Operations are `--` flags, so real words like `build` and `where` can be looked up.
Inflected forms are reduced to a base form (`walked` → `walk`).

Exit codes: `0` found, `1` not found (suggestions still print), `2` usage error.

The database is `anotherword.sqlite` beside the executable, or in the OS data
directory if that is not writable. Set `ANOTHERWORD_DB` to override.

## Data

[Princeton WordNet 3.1](https://wordnet.princeton.edu/). Antonyms are sparse:
WordNet records them directly for only some words, mostly adjectives.
