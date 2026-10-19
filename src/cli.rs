//! Command-line surface. Per DESIGN.md §5 there are no bare subcommands: the
//! sole positional is the word to look up, and every operation is a `--` flag,
//! so real words like "build" and "where" can still be looked up.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "anotherword",
    version,
    about = "Fast offline thesaurus",
    long_about = "Look up synonyms and antonyms for a word, offline, grouped by sense.\n\n\
                  Operations (--build, --where, --count) are flags so that real words \
                  such as `build` or `where` can themselves be looked up."
)]
pub struct Cli {
    /// The word to look up.
    pub word: Option<String>,

    // ---- lookup modifiers ----
    /// Also show related words (hypernyms, similar-to, see-also).
    #[arg(long)]
    pub related: bool,

    /// Cap the number of words in each synonym/antonym list.
    #[arg(long, value_name = "N")]
    pub max: Option<usize>,

    /// Language code (default: en).
    #[arg(long, default_value = "en")]
    pub lang: String,

    /// Hard-wrap output to N columns (default: no wrapping; let the terminal reflow).
    #[arg(long, value_name = "N")]
    pub width: Option<usize>,

    /// Emit structured JSON instead of text.
    #[arg(long)]
    pub json: bool,

    /// Disable colored output. Color is on by default when writing to a terminal.
    #[arg(long = "no-color")]
    pub no_color: bool,

    // ---- output-section toggles ----
    /// Suppress synonym lines.
    #[arg(long = "no-synonyms")]
    pub no_synonyms: bool,

    /// Suppress antonym lines.
    #[arg(long = "no-antonyms")]
    pub no_antonyms: bool,

    /// How many did-you-mean suggestions to show on a miss.
    #[arg(long, default_value_t = 8, value_name = "N")]
    pub max_suggestions: usize,

    // ---- operations (mutually exclusive with a lookup) ----
    /// (Re)build the SQLite database from WordNet.
    #[arg(long)]
    pub build: bool,

    /// With --build, rebuild even if a database already exists.
    #[arg(long)]
    pub force: bool,

    /// Print the resolved database path and exit.
    #[arg(long)]
    pub r#where: bool,

    /// Print word/sense counts and the absolute database path.
    #[arg(long)]
    pub count: bool,
}

impl Cli {
    /// How many mutually exclusive operation modes were requested.
    pub fn operation_count(&self) -> usize {
        [self.build, self.r#where, self.count]
            .iter()
            .filter(|b| **b)
            .count()
    }
}
