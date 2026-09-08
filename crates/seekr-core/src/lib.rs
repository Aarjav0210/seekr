//! seekr-core: the UI-free foundation for seekr.
//!
//! Everything a front-end (TUI, CLI, or an AI agent via MCP) needs lives here:
//! - [`item::Item`] / [`item::Kind`]: the things you can find.
//! - [`query::Query`]: the tiny query language (`app: whatsapp`, `doc: offer letter`, `in:Downloads`, `ext:pdf`).
//! - [`providers`]: sources of items (application bundles, files on disk).
//! - [`index::Index`]: SQLite-backed persistent index + usage (frecency) log.
//! - [`search::Searcher`]: incremental fuzzy matching (nucleo) with kind/frecency-aware re-ranking.
//! - [`actions`]: open / reveal / copy, the primitive "tools".
//! - [`doctor`]: diagnostics for Spotlight and our own index.

pub mod actions;
pub mod config;
pub mod doctor;
pub mod index;
pub mod indexer;
pub mod item;
pub mod providers;
pub mod query;
pub mod search;

pub use config::Config;
pub use index::{Index, Usage};
pub use item::{Item, Kind};
pub use query::Query;
pub use search::{Hit, Searcher};
pub use nucleo::Injector;

/// Current unix time in seconds.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
