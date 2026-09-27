//! Orchestrates providers → index.
use crate::config::Config;
use crate::index::Index;
use crate::item::Item;
use crate::providers::{apps, files, places};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

pub const SOURCE_APPS: &str = "apps";
pub const SOURCE_LSREGISTER: &str = "lsregister";
pub const SOURCE_WALK: &str = "walk";
pub const SOURCE_PLACES: &str = "places";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexReport {
    pub apps: usize,
    pub lsregister_apps: usize,
    /// Well-known folders and the roots themselves.
    pub places: usize,
    pub files: usize,
    pub folders: usize,
    pub walk_errors: usize,
    pub elapsed_ms: u128,
}

/// Rebuild the index. If `full`, also consult LaunchServices for apps in odd places.
/// `on_item` sees every discovered item as it is found (used to stream into a live matcher).
pub fn reindex(
    cfg: &Config,
    idx: &mut Index,
    full: bool,
    mut on_item: impl FnMut(&Item),
) -> Result<IndexReport> {
    let start = Instant::now();
    let mut report = IndexReport::default();

    let app_items = apps::scan_app_dirs(cfg);
    for it in &app_items {
        on_item(it);
    }
    report.apps = app_items.len();
    idx.replace_items(SOURCE_APPS, &app_items)?;

    if full {
        let ls = apps::scan_launch_services()?;
        let known: HashSet<_> = app_items.iter().map(|i| i.path.clone()).collect();
        let extra: Vec<Item> = ls
            .into_iter()
            .filter(|i| !known.contains(&i.path))
            .collect();
        for it in &extra {
            on_item(it);
        }
        report.lsregister_apps = extra.len();
        idx.replace_items(SOURCE_LSREGISTER, &extra)?;
    }

    let place_items = places::scan(cfg);
    for it in &place_items {
        on_item(it);
    }
    report.places = place_items.len();
    let place_paths: HashSet<PathBuf> = place_items.iter().map(|i| i.path.clone()).collect();

    let mut walked: Vec<Item> = Vec::with_capacity(100_000);
    let mut skipped = 0;
    let stats = files::walk_files(cfg, |it| {
        // A place inside a root (e.g. with `~` as a root) is indexed as the place.
        if place_paths.contains(&it.path) {
            skipped += 1;
            return;
        }
        on_item(&it);
        walked.push(it);
    });
    report.files = stats.files;
    report.folders = stats.folders - skipped;
    report.walk_errors = stats.errors;
    idx.replace_items(SOURCE_WALK, &walked)?;
    idx.replace_items(SOURCE_PLACES, &place_items)?;
    idx.mark_indexed()?;

    report.elapsed_ms = start.elapsed().as_millis();
    Ok(report)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RefreshReport {
    pub added: usize,
    pub removed: usize,
    pub updated: usize,
    pub scanned: usize,
    pub elapsed_ms: u128,
}

impl RefreshReport {
    pub fn summary(&self) -> String {
        format!(
            "refreshed in {:.1}s · +{} added · −{} removed · {} updated",
            self.elapsed_ms as f64 / 1000.0,
            self.added,
            self.removed,
            self.updated
        )
    }
}

/// Incrementally bring the index up to date: re-scan apps and re-walk the roots,
/// then write only the difference (new, changed, vanished items) to the database.
///
/// `apps` is the live application scan (callers usually already have it).
/// `on_new` sees every item that was not previously indexed, as it is found.
pub fn refresh(
    cfg: &Config,
    idx: &mut Index,
    apps: &[Item],
    mut on_new: impl FnMut(&Item),
) -> Result<RefreshReport> {
    let start = Instant::now();
    let cached = idx.load_fingerprints()?;
    let mut seen: HashSet<PathBuf> = HashSet::with_capacity(cached.len() + 1024);
    let mut report = RefreshReport::default();

    // Classify one freshly discovered item against the cache. Flags are
    // compared too, so a folder indexed by an older walk becomes a place.
    let mut classify = |it: &Item,
                        added: &mut Vec<Item>,
                        updated: &mut Vec<Item>,
                        seen: &mut HashSet<PathBuf>| {
        seen.insert(it.path.clone());
        match cached.get(&it.path) {
            None => {
                on_new(it);
                added.push(it.clone());
            }
            Some(&fp) if fp != (it.modified, it.size, it.flags) => {
                updated.push(it.clone());
            }
            _ => {}
        }
    };

    let mut app_added = Vec::new();
    let mut app_updated = Vec::new();
    for a in apps {
        classify(a, &mut app_added, &mut app_updated, &mut seen);
    }

    let place_items = places::scan(cfg);
    let place_paths: HashSet<PathBuf> = place_items.iter().map(|i| i.path.clone()).collect();
    let mut place_added = Vec::new();
    let mut place_updated = Vec::new();
    for p in &place_items {
        classify(p, &mut place_added, &mut place_updated, &mut seen);
    }

    let mut walk_added = Vec::new();
    let mut walk_updated = Vec::new();
    let mut skipped = 0;
    let stats = files::walk_files(cfg, |it| {
        if place_paths.contains(&it.path) {
            skipped += 1;
            return;
        }
        classify(&it, &mut walk_added, &mut walk_updated, &mut seen);
    });
    report.scanned = apps.len() + place_items.len() + stats.files + stats.folders - skipped;

    // Anything cached under a scanned location, or at a place's path, that we did
    // not see has vanished. Items outside the scanned locations (e.g. apps found
    // via LaunchServices) are kept.
    let scopes: Vec<PathBuf> = cfg
        .root_paths()
        .into_iter()
        .chain(cfg.app_dir_paths())
        .collect();
    let place_candidates: HashSet<PathBuf> = places::candidates(cfg).into_iter().collect();
    let removed: Vec<PathBuf> = cached
        .keys()
        .filter(|p| {
            !seen.contains(*p)
                && (scopes.iter().any(|s| p.starts_with(s)) || place_candidates.contains(*p))
        })
        .cloned()
        .collect();

    report.added = app_added.len() + place_added.len() + walk_added.len();
    report.updated = app_updated.len() + place_updated.len() + walk_updated.len();
    report.removed = removed.len();

    app_added.extend(app_updated);
    place_added.extend(place_updated);
    walk_added.extend(walk_updated);
    idx.upsert_items(SOURCE_APPS, &app_added)?;
    idx.upsert_items(SOURCE_PLACES, &place_added)?;
    idx.upsert_items(SOURCE_WALK, &walk_added)?;
    idx.delete_paths(&removed)?;
    idx.mark_indexed()?;

    report.elapsed_ms = start.elapsed().as_millis();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_dir;
    use std::fs;

    /// Two roots, one inside the other, plus a root that is about to vanish.
    fn setup(tag: &str) -> (PathBuf, Config, Index) {
        let tmp = temp_dir(tag);
        let outer = tmp.join("Outer");
        fs::create_dir_all(outer.join("Inner/sub")).unwrap();
        fs::create_dir_all(tmp.join("Solo")).unwrap();
        fs::write(outer.join("a.txt"), "a").unwrap();
        fs::write(outer.join("Inner/b.txt"), "b").unwrap();
        let cfg = Config {
            roots: ["Outer", "Outer/Inner", "Solo"]
                .iter()
                .map(|r| tmp.join(r).to_string_lossy().into_owned())
                .collect(),
            app_dirs: Vec::new(),
            ..Config::default()
        };
        let idx = Index::open(&tmp.join("index.db")).unwrap();
        (tmp, cfg, idx)
    }

    fn item_at(idx: &Index, p: &std::path::Path) -> Option<Item> {
        idx.load_items(None).unwrap().into_iter().find(|i| i.path == p)
    }

    #[test]
    fn refresh_adds_places_to_an_index_built_without_them() {
        let (tmp, cfg, mut idx) = setup("refresh-places");
        // What older versions stored: the walk only, where the nested root
        // `Inner` is a plain folder and no root is an item of its own.
        let mut walked = Vec::new();
        files::walk_files(&cfg, |it| walked.push(it));
        idx.replace_items(SOURCE_WALK, &walked).unwrap();
        let inner = tmp.join("Outer/Inner");
        assert!(!item_at(&idx, &inner).unwrap().is_place());
        assert!(item_at(&idx, &tmp.join("Outer")).is_none());

        let places = places::scan(&cfg);
        let r = refresh(&cfg, &mut idx, &[], |_| {}).unwrap();
        assert_eq!(r.added, places.len() - 1, "every place but the cached Inner is new");
        assert_eq!(r.removed, 0);
        for p in &places {
            assert!(item_at(&idx, &p.path).unwrap().is_place(), "{}", p.path.display());
        }
        let distinct: HashSet<_> = walked.iter().map(|i| &i.path).collect();
        assert_eq!(idx.count().unwrap(), distinct.len() + places.len() - 1);

        let again = refresh(&cfg, &mut idx, &[], |_| {}).unwrap();
        assert_eq!((again.added, again.removed), (0, 0));

        // A place that disappears leaves the index, even outside the walked roots.
        fs::remove_dir_all(tmp.join("Solo")).unwrap();
        let gone = refresh(&cfg, &mut idx, &[], |_| {}).unwrap();
        assert_eq!((gone.added, gone.removed), (0, 1));
        assert!(item_at(&idx, &tmp.join("Solo")).is_none());
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn reindex_stores_each_place_once() {
        let (tmp, cfg, mut idx) = setup("reindex-places");
        let r = reindex(&cfg, &mut idx, false, |_| {}).unwrap();
        assert_eq!(r.places, places::scan(&cfg).len());
        // Inner is walked as part of Outer too, but stored once, as a place;
        // besides the places only a.txt, b.txt and sub remain.
        assert_eq!(idx.count().unwrap(), r.places + 3);
        for root in ["Outer", "Outer/Inner", "Solo"] {
            assert!(item_at(&idx, &tmp.join(root)).unwrap().is_place(), "{root}");
        }
        let again = refresh(&cfg, &mut idx, &[], |_| {}).unwrap();
        assert_eq!((again.added, again.removed), (0, 0));
        fs::remove_dir_all(&tmp).ok();
    }
}
