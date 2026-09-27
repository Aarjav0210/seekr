//! Incremental fuzzy search over items, powered by nucleo, with seekr-specific re-ranking.
//!
//! Items are injected with three match columns:
//! 0. `name [alt]` — what the user's terms are matched against.
//! 1. `kind`       — exact-matched when a scope (`app:`) is active.
//! 2. `path`       — substring-matched for `in:` filters.
use crate::index::Usage;
use crate::item::{Item, Kind};
use crate::query::Query;
use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config as NucleoConfig, Injector, Matcher, Nucleo, Status, Utf32String};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

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
/// Shortest query that counts as naming a place.
const PLACE_MIN_PREFIX: usize = 2;

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

    pub fn total_items(&self) -> u32 {
        self.nucleo.snapshot().item_count()
    }

    pub fn matched_count(&self) -> u32 {
        self.nucleo.snapshot().matched_item_count()
    }

    /// Ranked results. Takes nucleo's top candidates and re-scores them with
    /// kind boosts, depth penalty and frecency.
    pub fn results(&mut self, limit: usize) -> Vec<Hit> {
        let now = crate::now_secs();
        let has_terms = !self.query.terms.is_empty();
        let Searcher {
            nucleo,
            matcher,
            usage,
            recent,
            query,
            ..
        } = self;
        let snap = nucleo.snapshot();
        let n = snap.matched_item_count();

        if !has_terms {
            // Empty terms: recents first (filtered by scope/in/ext), then whatever
            // nucleo matched (apps are injected first, so they come next) by name.
            let mut out: Vec<Hit> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for r in recent.iter() {
                if query.scope.is_some_and(|k| k != r.kind) {
                    continue;
                }
                if let Some(d) = &query.in_dir {
                    if !r.path_display().to_lowercase().contains(&d.to_lowercase()) {
                        continue;
                    }
                }
                if let Some(e) = &query.ext {
                    if !r.name.to_lowercase().ends_with(&format!(".{e}")) {
                        continue;
                    }
                }
                if seen.insert(r.path.clone()) {
                    let u = usage.get(&r.path).copied().unwrap_or_default();
                    out.push(Hit {
                        item: r.clone(),
                        score: frecency(Some(&u), now),
                        indices: vec![],
                    });
                }
                if out.len() >= limit {
                    break;
                }
            }
            let k = (limit * 3).min(n as usize) as u32;
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
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.item.name.to_lowercase().cmp(&b.item.name.to_lowercase()))
            });
            out.extend(rest);
            out.truncate(limit);
            return out;
        }

        let k = (limit * 5).clamp(200, 1000).min(n as usize) as u32;
        let pattern = snap.pattern().column_pattern(0);
        let scoped = query.scope.is_some();
        let terms = query.terms.trim().to_lowercase();
        let names_place = terms.chars().count() >= PLACE_MIN_PREFIX;
        let mut indices = Vec::new();
        let mut hits: Vec<Hit> = snap
            .matched_items(0..k)
            .map(|it| {
                indices.clear();
                let hay = it.matcher_columns[0].slice(..);
                let base = pattern
                    .indices(hay, matcher, &mut indices)
                    .unwrap_or(0) as f32;
                let name_len = it.data.name.chars().count() as u32;
                indices.sort_unstable();
                indices.dedup();
                let idx: Vec<u32> = indices.iter().copied().filter(|&i| i < name_len).collect();
                let mut kind_mult = if scoped { 1.0 } else { it.data.kind.base_boost() };
                let names_this_place = names_place
                    && it.data.is_place()
                    && name_matches(&it.data, |n| n.starts_with(&terms));
                if names_this_place {
                    kind_mult *= PLACE_BOOST;
                } else if it.data.kind == Kind::App && name_matches(&it.data, |n| n == terms) {
                    kind_mult *= EXACT_APP_BOOST;
                }
                let depth_pen = 1.0 - 0.015 * (it.data.depth().saturating_sub(3).min(10) as f32);
                // Source trees are noise unless you asked for code.
                let repo_pen = if it.data.in_repo() && query.scope != Some(Kind::Code) {
                    0.55
                } else {
                    1.0
                };
                let frec = frecency(usage.get(&it.data.path), now);
                Hit {
                    item: it.data.clone(),
                    score: base * kind_mult * depth_pen * repo_pen + frec,
                    indices: idx,
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.item.name.len().cmp(&b.item.name.len()))
                .then_with(|| b.item.modified.cmp(&a.item.modified))
        });
        hits.truncate(limit);
        hits
    }
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

/// Kinds that should be loaded for a query, honouring lazy kinds.
pub fn kinds_for_query(q: &Query, lazy: &[Kind]) -> Vec<Kind> {
    match q.scope {
        Some(k) => vec![k],
        None => Kind::ALL
            .iter()
            .copied()
            .filter(|k| !lazy.contains(k))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn searcher_with(names: &[&str]) -> Searcher {
        let mut s = Searcher::new(HashMap::new(), Vec::new());
        for n in names {
            s.push(Item::new(Kind::App, *n, format!("/Applications/{n}.app")));
        }
        s
    }

    fn settle(s: &mut Searcher) {
        for _ in 0..200 {
            if !s.tick(10).running {
                return;
            }
        }
        panic!("matcher did not settle");
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
