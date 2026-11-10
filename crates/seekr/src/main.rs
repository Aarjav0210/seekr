mod cli;
#[cfg(target_os = "macos")]
mod gui;
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

    /// Initial query for the interactive UI (e.g. `seekr app: whatsapp`).
    #[arg(trailing_var_arg = true)]
    query: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Interactive fuzzy finder (default when no subcommand is given).
    Tui {
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
    /// Define a word with the macOS dictionary (offline). Exits non-zero when there is no entry.
    Define {
        word: String,
        /// Emit JSON: headword, pronunciation, parts of speech with senses, origin, raw text.
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
    /// The launcher: seekr in a window of its own that a hotkey brings up from
    /// any app (what seekr.app runs, one copy at a time). The hotkey is
    /// --hotkey, else `defaults read io.github.aarjav0210.seekr hotkey`, else cmd+space.
    #[cfg(target_os = "macos")]
    App {
        /// Modifiers and key joined with "+", e.g. cmd+space, ctrl+space, cmd+shift+k.
        #[arg(long)]
        hotkey: Option<String>,
        /// Show the window right away instead of waiting for the hotkey.
        #[arg(long)]
        show: bool,
        /// Draw the window into this PNG and exit, without showing it.
        #[arg(long, value_name = "PNG", hide = true)]
        snapshot: Option<PathBuf>,
        /// The query typed for --snapshot.
        #[arg(long, default_value = "", hide = true)]
        query: String,
        /// Only check that the hotkey is one seekr understands (for install.sh).
        #[arg(long, hide = true)]
        check: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        #[cfg(target_os = "macos")]
        None if cli.query.is_empty() && opened_as_app() => gui::run(gui::Options {
            hotkey: None,
            show: false,
            snapshot: None,
            query: String::new(),
            check: false,
        }),
        None => tui::run(cli.query.join(" ")),
        Some(Cmd::Tui { query }) => tui::run(query.join(" ")),
        #[cfg(target_os = "macos")]
        Some(Cmd::App { hotkey, show, snapshot, query, check }) => {
            gui::run(gui::Options { hotkey, show, snapshot, query, check })
        }
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
        Some(Cmd::Define { word, json }) => cli::define(&word, json),
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

/// Started by double-clicking seekr.app (or `open`): LaunchServices runs the
/// binary inside the bundle, with launchd as its parent and no terminal.
#[cfg(target_os = "macos")]
fn opened_as_app() -> bool {
    use std::io::IsTerminal;
    let in_bundle = std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"));
    in_bundle && std::os::unix::process::parent_id() == 1 && !std::io::stdin().is_terminal()
}
