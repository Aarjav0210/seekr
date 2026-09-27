mod cli;
mod launcher;
mod loader;
mod tui;
mod util;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// seekr — fast local search for macOS. Type to find apps, documents and folders; press ⏎ to open.
#[derive(Parser, Debug)]
#[command(name = "seekr", version, about, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    /// Stay alive after Enter/Esc: hide the launcher window instead of quitting, so the
    /// next hotkey press (Cmd+Space) shows the UI instantly. Used by contrib/ (Hammerspoon + kitty).
    #[arg(long)]
    resident: bool,

    /// Initial query for the interactive UI (e.g. `seekr app: whatsapp`).
    #[arg(trailing_var_arg = true)]
    query: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Interactive fuzzy finder (default when no subcommand is given).
    Tui {
        /// See `seekr --help`.
        #[arg(long)]
        resident: bool,
        #[arg(trailing_var_arg = true)]
        query: Vec<String>,
    },
    /// One-shot search; prints a table or JSON. Suitable for scripts and agents.
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        /// Maximum number of results.
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// Emit JSON (array of {item, score}).
        #[arg(long)]
        json: bool,
        /// Include lazily-loaded kinds (e.g. source files) even without a scope.
        #[arg(long)]
        all: bool,
        /// Refresh the index (re-walk the roots) before searching.
        #[arg(long)]
        fresh: bool,
    },
    /// Incrementally refresh the index: pick up new, changed and deleted files.
    Refresh {
        #[arg(long)]
        json: bool,
    },
    /// Open a path (records usage for ranking). `--reveal` shows it in Finder instead.
    Open {
        path: PathBuf,
        #[arg(long)]
        reveal: bool,
    },
    /// Rebuild the index from scratch. `--full` also consults LaunchServices for apps in unusual places.
    Index {
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },
    /// Diagnose Spotlight and the seekr index.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// List the kinds/scopes the query language understands.
    Kinds {
        #[arg(long)]
        json: bool,
    },
    /// List the apps a query can be handed to (⇥ in the finder): Arc and Spotify by default.
    Searches {
        #[arg(long)]
        json: bool,
    },
    /// Search inside an app: `seekr search-in spotify daft punk`. Without terms, opens the app.
    SearchIn {
        /// Keyword of the search; `seekr searches` lists them.
        keyword: String,
        /// What to search for.
        terms: Vec<String>,
        /// Print the URL instead of opening it.
        #[arg(long)]
        print: bool,
        /// Emit JSON: keyword, name, app, terms, url and the `open` command.
        #[arg(long)]
        json: bool,
    },
    /// Evaluate a calculation: `seekr calc '(1+2)^10/7'`. Exits non-zero when it isn't one.
    Calc {
        /// The expression; several words are joined with spaces. Quote it in the shell.
        #[arg(required = true, allow_hyphen_values = true)]
        expr: Vec<String>,
        /// Emit JSON: input, expr, value, raw and display (or input and error).
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        None => tui::run(cli.query.join(" "), cli.resident),
        Some(Cmd::Tui { resident, query }) => tui::run(query.join(" "), resident),
        Some(Cmd::Search {
            query,
            limit,
            json,
            all,
            fresh,
        }) => cli::search(&query.join(" "), limit, json, all, fresh),
        Some(Cmd::Refresh { json }) => cli::refresh(json),
        Some(Cmd::Open { path, reveal }) => cli::open(&path, reveal),
        Some(Cmd::Index { full, json }) => cli::index(full, json),
        Some(Cmd::Doctor { json }) => cli::doctor(json),
        Some(Cmd::Kinds { json }) => cli::kinds(json),
        Some(Cmd::Searches { json }) => cli::searches(json),
        Some(Cmd::SearchIn {
            keyword,
            terms,
            print,
            json,
        }) => cli::search_in(&keyword, &terms.join(" "), print, json),
        Some(Cmd::Calc { mut expr, json }) => {
            // `-5+3` has to reach the expression, so hyphen values are allowed,
            // which makes a trailing `--json` look like part of it.
            let json = json || expr.iter().any(|w| w == "--json");
            expr.retain(|w| w != "--json");
            cli::calc(&expr.join(" "), json)
        }
    }
}
