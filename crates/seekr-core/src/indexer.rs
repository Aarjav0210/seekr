//! Orchestrates providers → index.
use crate::config::Config;
use crate::index::Index;
use crate::item::Item;
use crate::providers::{apps, files};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

pub const SOURCE_APPS: &str = "apps";
pub const SOURCE_LSREGISTER: &str = "lsregister";
pub const SOURCE_WALK: &str = "walk";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexReport {
    pub apps: usize,
    pub lsregister_apps: usize,
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
        let known: std::collections::HashSet<_> =
            app_items.iter().map(|i| i.path.clone()).collect();
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

    let mut walked: Vec<Item> = Vec::with_capacity(100_000);
    let stats = files::walk_files(cfg, |it| {
        on_item(&it);
        walked.push(it);
    });
    report.files = stats.files;
    report.folders = stats.folders;
    report.walk_errors = stats.errors;
    idx.replace_items(SOURCE_WALK, &walked)?;
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

    // Classify one freshly discovered item against the cache.
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
            Some(&(modified, size)) if modified != it.modified || size != it.size => {
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

    let mut walk_added = Vec::new();
    let mut walk_updated = Vec::new();
    let stats = files::walk_files(cfg, |it| {
        classify(&it, &mut walk_added, &mut walk_updated, &mut seen);
    });
    report.scanned = apps.len() + stats.files + stats.folders;

    // Anything cached under a scanned location that we did not see has vanished.
    // Items outside the scanned locations (e.g. apps found via LaunchServices) are kept.
    let scopes: Vec<PathBuf> = cfg
        .root_paths()
        .into_iter()
        .chain(cfg.app_dir_paths())
        .collect();
    let removed: Vec<PathBuf> = cached
        .keys()
        .filter(|p| !seen.contains(*p) && scopes.iter().any(|s| p.starts_with(s)))
        .cloned()
        .collect();

    report.added = app_added.len() + walk_added.len();
    report.updated = app_updated.len() + walk_updated.len();
    report.removed = removed.len();

    app_added.extend(app_updated);
    walk_added.extend(walk_updated);
    idx.upsert_items(SOURCE_APPS, &app_added)?;
    idx.upsert_items(SOURCE_WALK, &walk_added)?;
    idx.delete_paths(&removed)?;
    idx.mark_indexed()?;

    report.elapsed_ms = start.elapsed().as_millis();
    Ok(report)
}
