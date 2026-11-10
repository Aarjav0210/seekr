//! Incremental fuzzy search over items, powered by nucleo, with seekr-specific re-ranking.
//!
//! Items are injected with three match columns:
//! 0. `name [alt]` — what the user's terms are matched against.
//! 1. `kind`       — exact-matched when a scope (`app:`) is active.
//! 2. `path`       — substring-matched for `in:` filters.
use crate::config::Config;
use crate::index::Usage;
use crate::item::{Item, Kind};
use crate::query::Query;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config as NucleoConfig, Injector, Matcher, Nucleo, Status, Utf32String};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub item: Item,
    pub score: f32,
    /// Character indices in `item.name` that matched (for highlighting).
    #[serde(skip)]
    pub indices: Vec<u32>,
}

pub struct Searcher {
    nucleo: Nucleo<Item>,
    matcher: Matcher,
    usage: HashMap<PathBuf, Usage>,
    recent: Vec<Item>,
    query: Query,
    col0: String,
}

/// Weight on a well-known folder (see [`Item::is_place`]) whose name starts
/// with the query, lifting it above apps that merely contain the word:
/// `desk` puts ~/Desktop before GitHub Desktop.
const PLACE_BOOST: f32 = 1.5;
/// Weight on an app named exactly what was typed, keeping it ahead of a
/// same-named place: `music` puts Music.app first, then ~/Music.
const EXACT_APP_BOOST: f32 = 1.2;
/// Shortest query that counts as naming a place or a settings pane.
const PLACE_MIN_PREFIX: usize = 2;
/// Weight on a settings pane named exactly what was typed (or one of its
/// extra words: `dark mode`), lifting it above apps that merely start with
/// it, but not above an app of that exact name.
const EXACT_SETTING_BOOST: f32 = 1.35;
/// Weight on a settings pane whose name starts with the query: ahead of
/// documents and folders, still behind an app that starts with it too
/// (`key`: Keynote, then Keyboard).
const SETTING_PREFIX_BOOST: f32 = 1.2;

fn fill(item: &Item, cols: &mut [Utf32String]) {
    cols[0] = Utf32String::from(item.search_text().as_str());
    cols[1] = Utf32String::from(item.kind.as_str());
    cols[2] = Utf32String::from(item.path_display().as_str());
}

impl Searcher {
    pub fn new(usage: HashMap<PathBuf, Usage>, recent: Vec<Item>) -> Self {
        let nucleo = Nucleo::new(NucleoConfig::DEFAULT, Arc::new(|| {}), None, 3);
        Searcher {
            nucleo,
            matcher: Matcher::new(NucleoConfig::DEFAULT),
            usage,
            recent,
            query: Query::default(),
            col0: String::new(),
        }
    }

    pub fn injector(&self) -> Injector<Item> {
        self.nucleo.injector()
    }

    pub fn push(&self, item: Item) {
        self.nucleo.injector().push(item, fill);
    }

    /// Push through a detached injector (from another thread).
    pub fn inject(inj: &Injector<Item>, item: Item) {
        inj.push(item, fill);
    }

    pub fn record_use(&mut self, item: &Item, usage: Usage) {
        self.usage.insert(item.path.clone(), usage);
        self.recent.retain(|r| r.path != item.path);
        self.recent.insert(0, item.clone());
    }

    pub fn query(&self) -> &Query {
        &self.query
    }

    pub fn set_query(&mut self, q: Query) {
        let mut col0 = q.terms.clone();
        if let Some(ext) = &q.ext {
            if !col0.is_empty() {
                col0.push(' ');
            }
            col0.push_str(&format!(".{ext}$"));
        }
        let append = col0.starts_with(&self.col0) && !self.col0.is_empty();
        self.nucleo.pattern.reparse(
            0,
            &col0,
            CaseMatching::Ignore,
            Normalization::Smart,
            append,
        );
        let col1 = q
            .scope
            .map(|k| format!("^{}$", k.as_str()))
            .unwrap_or_default();
        self.nucleo
            .pattern
            .reparse(1, &col1, CaseMatching::Ignore, Normalization::Never, false);
        let col2 = q
            .in_dir
            .as_deref()
            .map(|d| format!("'{d}"))
            .unwrap_or_default();
        self.nucleo
            .pattern
            .reparse(2, &col2, CaseMatching::Ignore, Normalization::Smart, false);
        self.col0 = col0;
        self.query = q;
    }

    /// Advance the matcher. Returns whether results changed / are still being computed.
    pub fn tick(&mut self, timeout_ms: u64) -> Status {
        self.nucleo.tick(timeout_ms)
    }

    /// Tick until the matcher has caught up with the query and every item
    /// pushed so far, for callers without an event loop (the one-shot CLI,
    /// tests). Gives up after `max`; returns whether it caught up.
    pub fn settle(&mut self, max: Duration) -> bool {
        let deadline = Instant::now() + max;
        while self.tick(10).running {
            if Instant::now() >= deadline {
                return false;
            }
        }
        true
    }

    pub fn total_items(&self) -> u32 {
        self.nucleo.snapshot().item_count()
    }

    pub fn matched_count(&self) -> u32 {
        self.nucleo.snapshot().matched_item_count()
    }

    /// Ranked results: recent items for a query without terms, else nucleo's
    /// top candidates re-scored with kind boosts, depth penalty and frecency.
    pub fn results(&mut self, limit: usize) -> Vec<Hit> {
        if self.query.terms.is_empty() {
            self.browse(limit)
        } else {
            self.ranked(limit)
        }
    }

    /// No terms: recently used items first (filtered like the query), then
    /// whatever nucleo matched (apps are injected first, so they come next),
    /// best kind first, then by name.
    fn browse(&self, limit: usize) -> Vec<Hit> {
        let now = crate::now_secs();
        let mut seen = HashSet::new();
        let mut out: Vec<Hit> = self
            .recent
            .iter()
            .filter(|r| self.query.matches_filters(r) && seen.insert(r.path.clone()))
            .take(limit)
            .map(|r| Hit {
                item: r.clone(),
                score: frecency(self.usage.get(&r.path), now),
                indices: vec![],
            })
            .collect();
        let snap = self.nucleo.snapshot();
        let k = (limit * 3).min(snap.matched_item_count() as usize) as u32;
        let mut rest: Vec<Hit> = snap
            .matched_items(0..k)
            .filter(|it| !seen.contains(&it.data.path))
            .map(|it| Hit {
                item: it.data.clone(),
                score: it.data.kind.base_boost(),
                indices: vec![],
            })
            .collect();
        rest.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.item.name.to_lowercase().cmp(&b.item.name.to_lowercase()))
        });
        out.extend(rest);
        out.truncate(limit);
        out
    }

    fn ranked(&mut self, limit: usize) -> Vec<Hit> {
        let now = crate::now_secs();
        let Searcher {
            nucleo,
            matcher,
            usage,
            query,
            ..
        } = self;
        let snap = nucleo.snapshot();
        let k = (limit * 5).clamp(200, 1000).min(snap.matched_item_count() as usize) as u32;
        let pattern = snap.pattern().column_pattern(0);
        let terms = query.terms.trim().to_lowercase();
        let mut indices = Vec::new();
        let mut hits: Vec<Hit> = snap
            .matched_items(0..k)
            .map(|it| {
                indices.clear();
                let hay = it.matcher_columns[0].slice(..);
                let base = pattern
                    .indices(hay, matcher, &mut indices)
                    .unwrap_or(0) as f32;
                // Highlight the name only; the alt name follows it in the haystack.
                let name_len = it.data.name.chars().count() as u32;
                indices.sort_unstable();
                indices.dedup();
                Hit {
                    item: it.data.clone(),
                    score: weigh(base, it.data, query, &terms) + frecency(usage.get(&it.data.path), now),
                    indices: indices.iter().copied().filter(|&i| i < name_len).collect(),
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.item.name.len().cmp(&b.item.name.len()))
                .then_with(|| b.item.modified.cmp(&a.item.modified))
        });
        hits.truncate(limit);
        hits
    }
}

/// `base`, the fuzzy score, times the item's weights: its kind (unless the
/// query is scoped), a boost when `terms` name a place, a settings pane or
/// exactly an app, and penalties for deep paths and for source trees.
fn weigh(base: f32, item: &Item, query: &Query, terms: &str) -> f32 {
    let mut kind_mult = if query.scope.is_some() { 1.0 } else { item.kind.base_boost() };
    let named = terms.chars().count() >= PLACE_MIN_PREFIX;
    let names_place = named && item.is_place() && name_matches(item, |n| n.starts_with(terms));
    if names_place {
        kind_mult *= PLACE_BOOST;
    } else if item.kind == Kind::App && name_matches(item, |n| n == terms) {
        kind_mult *= EXACT_APP_BOOST;
    } else if named && item.is_setting() {
        if item.name.to_lowercase() == terms || setting_words(item).any(|w| w == terms) {
            kind_mult *= EXACT_SETTING_BOOST;
        } else if item.name.to_lowercase().starts_with(terms) {
            kind_mult *= SETTING_PREFIX_BOOST;
        }
    }
    let depth_pen = 1.0 - 0.015 * (item.depth().saturating_sub(3).min(10) as f32);
    // Source trees are noise unless you asked for code.
    let repo_pen = if item.in_repo() && query.scope != Some(Kind::Code) {
        0.55
    } else {
        1.0
    };
    base * kind_mult * depth_pen * repo_pen
}

/// A settings pane's extra words (`dark mode, accent colour`), lowercased.
fn setting_words(item: &Item) -> impl Iterator<Item = String> + '_ {
    item.alt
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
}

/// Whether the item's name or alt name, lowercased, satisfies `pred`.
fn name_matches(item: &Item, pred: impl Fn(&str) -> bool) -> bool {
    pred(&item.name.to_lowercase())
        || item.alt.as_ref().is_some_and(|a| pred(&a.to_lowercase()))
}

/// Frecency bonus: more uses and more recent uses score higher. Capped so a
/// frequently opened item can't beat a clearly better textual match by miles.
pub fn frecency(u: Option<&Usage>, now: i64) -> f32 {
    let Some(u) = u else { return 0.0 };
    let age = (now - u.last_used).max(0);
    let mult = if age < 3600 {
        4.0
    } else if age < 86_400 {
        3.0
    } else if age < 7 * 86_400 {
        2.0
    } else if age < 30 * 86_400 {
        1.5
    } else {
        1.0
    };
    (u.count.min(20) as f32 * mult * 6.0).min(250.0)
}

/// Kinds to load for a query: its scope, else every kind that is not lazy.
pub fn kinds_for_query(q: &Query, cfg: &Config) -> Vec<Kind> {
    match q.scope {
        Some(k) => vec![k],
        None => cfg.eager_kinds(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn searcher_with(names: &[&str]) -> Searcher {
        let s = Searcher::new(HashMap::new(), Vec::new());
        for n in names {
            s.push(Item::new(Kind::App, *n, format!("/Applications/{n}.app")));
        }
        s
    }

    fn settle(s: &mut Searcher) {
        assert!(s.settle(Duration::from_secs(2)), "matcher did not settle");
    }

    fn names(s: &mut Searcher, raw: &str) -> Vec<String> {
        s.set_query(Query::parse(raw));
        settle(s);
        s.results(10).into_iter().map(|h| h.item.name).collect()
    }

    fn place(name: &str, path: &str) -> Item {
        let mut it = Item::new(Kind::Folder, name, path);
        it.flags = crate::item::FLAG_PLACE;
        it
    }

    /// A realistic mix: places, apps whose names contain or equal a place's
    /// name, and files and folders that merely mention it.
    fn searcher_with_places() -> Searcher {
        let s = Searcher::new(HashMap::new(), Vec::new());
        for app in ["GitHub Desktop", "Neo4j Desktop", "Music", "Safari", "Notes"] {
            s.push(Item::new(Kind::App, app, format!("/Applications/{app}.app")));
        }
        let mut home = place("jdoe", "/Users/jdoe");
        home.alt = Some("Home".into());
        s.push(home);
        s.push(place("Desktop", "/Users/jdoe/Desktop"));
        s.push(place("Downloads", "/Users/jdoe/Downloads"));
        s.push(place("Music", "/Users/jdoe/Music"));
        s.push(place("Applications", "/Applications"));
        s.push(place("iCloud Drive", "/Users/jdoe/Library/Mobile Documents/com~apple~CloudDocs"));
        for (kind, name, path) in [
            (Kind::Video, "TV Static [DOWNLOAD LINK].mp4", "/Users/jdoe/Downloads/TV Static [DOWNLOAD LINK].mp4"),
            (Kind::Doc, "Downloads checklist.pdf", "/Users/jdoe/Documents/Downloads checklist.pdf"),
            (Kind::Folder, "Downloads", "/Users/jdoe/Documents/old/Downloads"),
            (Kind::Folder, "Desktop Backup", "/Users/jdoe/Documents/Desktop Backup"),
            (Kind::Folder, "Music", "/Users/jdoe/Music/Music"),
            (Kind::Doc, "Safari notes.md", "/Users/jdoe/Documents/Safari notes.md"),
        ] {
            s.push(Item::new(kind, name, path));
        }
        s
    }

    fn top(s: &mut Searcher, raw: &str, n: usize) -> Vec<String> {
        s.set_query(Query::parse(raw));
        settle(s);
        s.results(n)
            .into_iter()
            .map(|h| format!("{} {}", h.item.kind.as_str(), h.item.path.display()))
            .collect()
    }

    #[test]
    fn a_named_place_is_the_top_hit() {
        let mut s = searcher_with_places();
        for q in ["downloads", "Downl", "DOWNLOADS"] {
            assert_eq!(top(&mut s, q, 1), ["folder /Users/jdoe/Downloads"], "{q}");
        }
        assert_eq!(top(&mut s, "desk", 1), ["folder /Users/jdoe/Desktop"]);
        assert_eq!(top(&mut s, "icloud", 1), ["folder /Users/jdoe/Library/Mobile Documents/com~apple~CloudDocs"]);
        assert_eq!(top(&mut s, "applications", 1), ["folder /Applications"]);
        assert_eq!(top(&mut s, "home", 1), ["folder /Users/jdoe"]);
        assert_eq!(top(&mut s, "jdoe", 1), ["folder /Users/jdoe"]);
        assert_eq!(top(&mut s, "dir: desk", 1), ["folder /Users/jdoe/Desktop"]);
    }

    #[test]
    fn apps_named_exactly_still_come_first() {
        let mut s = searcher_with_places();
        assert_eq!(
            top(&mut s, "music", 2),
            ["app /Applications/Music.app", "folder /Users/jdoe/Music"]
        );
        assert_eq!(top(&mut s, "safari", 1), ["app /Applications/Safari.app"]);
        assert_eq!(top(&mut s, "notes", 1), ["app /Applications/Notes.app"]);
    }

    fn searcher_with_settings() -> Searcher {
        let s = searcher_with_places();
        for app in ["Keynote", "DisplayLink Manager", "Monitor Control", "Sound Source", "System Settings"] {
            s.push(Item::new(Kind::App, app, format!("/Applications/{app}.app")));
        }
        for id in [
            "com.apple.Mouse-Settings.extension",
            "com.apple.Keyboard-Settings.extension",
            "com.apple.Displays-Settings.extension",
            "com.apple.Sound-Settings.extension",
            "com.apple.Appearance-Settings.extension",
            "com.apple.Desktop-Settings.extension",
        ] {
            s.push(crate::providers::settings::item(id).unwrap());
        }
        for (kind, name, path) in [
            (Kind::Doc, "mouse notes.md", "/Users/jdoe/Documents/mouse notes.md"),
            (Kind::Folder, "Mouse", "/Users/jdoe/Documents/Mouse"),
            (Kind::Image, "keyboard.png", "/Users/jdoe/Pictures/keyboard.png"),
            (Kind::Doc, "Dark Mode.pdf", "/Users/jdoe/Documents/Dark Mode.pdf"),
        ] {
            s.push(Item::new(kind, name, path));
        }
        s
    }

    #[test]
    fn a_settings_pane_is_found_by_its_name_or_its_words() {
        let mut s = searcher_with_settings();
        let mouse = "setting x-apple.systempreferences:com.apple.Mouse-Settings.extension";
        for q in ["mouse", "Mouse", "mou"] {
            assert_eq!(top(&mut s, q, 1), [mouse], "{q}");
        }
        assert_eq!(
            top(&mut s, "dark mode", 1),
            ["setting x-apple.systempreferences:com.apple.Appearance-Settings.extension"]
        );
        assert_eq!(
            top(&mut s, "dock", 1),
            ["setting x-apple.systempreferences:com.apple.Desktop-Settings.extension"]
        );
        // Scoped, and spelt the other ways.
        for q in ["setting: mouse", "settings: mouse", "prefs: mouse"] {
            assert_eq!(top(&mut s, q, 1), [mouse], "{q}");
        }
        assert!(top(&mut s, "settings:", 20).iter().all(|r| r.starts_with("setting ")));
    }

    #[test]
    fn apps_still_come_before_a_pane_they_share_a_prefix_with() {
        let mut s = searcher_with_settings();
        assert_eq!(
            top(&mut s, "key", 2),
            [
                "app /Applications/Keynote.app",
                "setting x-apple.systempreferences:com.apple.Keyboard-Settings.extension"
            ]
        );
        assert_eq!(
            top(&mut s, "keyb", 1),
            ["setting x-apple.systempreferences:com.apple.Keyboard-Settings.extension"]
        );
        // Named in full, the pane beats apps that merely start with the word.
        assert_eq!(
            top(&mut s, "sound", 2),
            [
                "setting x-apple.systempreferences:com.apple.Sound-Settings.extension",
                "app /Applications/Sound Source.app"
            ]
        );
        assert_eq!(top(&mut s, "settings", 1), ["app /Applications/System Settings.app"]);
    }

    #[test]
    fn a_single_letter_does_not_name_a_place() {
        let mut s = searcher_with_places();
        assert_ne!(top(&mut s, "d", 1), ["folder /Users/jdoe/Desktop"]);
        assert_ne!(top(&mut s, "d", 1), ["folder /Users/jdoe/Downloads"]);
    }

    #[test]
    fn uppercase_query_matches_mixed_case_name() {
        let mut s = searcher_with(&["Arc", "Safari"]);
        assert_eq!(names(&mut s, "ARC"), vec!["Arc"]);
        assert_eq!(names(&mut s, "arc"), vec!["Arc"]);
        assert_eq!(names(&mut s, "aRc"), vec!["Arc"]);
    }

    #[test]
    fn uppercase_query_still_matches_when_typed_incrementally() {
        let mut s = searcher_with(&["Arc", "Safari"]);
        // Each keystroke appends to the previous pattern; every step must keep "Arc".
        for q in ["A", "AR", "ARC"] {
            assert!(names(&mut s, q).contains(&"Arc".to_string()), "query {q:?} lost Arc");
        }
        assert_eq!(names(&mut s, "ARC"), vec!["Arc"]);
    }
}
