//! File and folder discovery: a parallel, gitignore-aware walk of the configured roots.
use crate::config::Config;
use crate::item::{mtime, Item, Kind, FLAG_IN_REPO, PACKAGE_EXTS};
use ignore::{WalkBuilder, WalkState};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, RwLock};

#[derive(Debug, Default, Clone)]
pub struct WalkStats {
    pub files: usize,
    pub folders: usize,
    pub errors: usize,
}

/// Walk all roots. `on_item` is invoked on the calling thread, in discovery order.
pub fn walk_files(cfg: &Config, mut on_item: impl FnMut(Item)) -> WalkStats {
    let roots = cfg.root_paths();
    let mut stats = WalkStats::default();
    let Some((first, rest)) = roots.split_first() else {
        return stats;
    };
    let skip: Arc<HashSet<String>> = Arc::new(cfg.skip_dirs.iter().cloned().collect());
    // Git repository roots seen so far. Parents are always visited before their
    // children, so a child can look its ancestors up here.
    let repos: Arc<RwLock<HashSet<PathBuf>>> = Arc::new(RwLock::new(HashSet::new()));
    let mut builder = WalkBuilder::new(first);
    for r in rest {
        builder.add(r);
    }
    builder
        .hidden(true)
        .git_ignore(cfg.respect_gitignore)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .ignore(false)
        .follow_links(false)
        .max_depth(Some(cfg.max_depth))
        .threads(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(12));

    let (tx, rx) = mpsc::channel::<Result<Item, ()>>();
    let walker = builder.build_parallel();
    std::thread::scope(|s| {
        s.spawn(move || {
            walker.run(|| {
                let tx = tx.clone();
                let skip = skip.clone();
                let repos = repos.clone();
                Box::new(move |res| {
                    let entry = match res {
                        Ok(e) => e,
                        Err(_) => {
                            let _ = tx.send(Err(()));
                            return WalkState::Continue;
                        }
                    };
                    if entry.depth() == 0 {
                        return WalkState::Continue;
                    }
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy();
                    let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                    let flags = if in_repo(path, &repos) { FLAG_IN_REPO } else { 0 };
                    if is_dir {
                        if skip.contains(name.as_ref()) {
                            return WalkState::Skip;
                        }
                        // Python virtualenvs have arbitrary names; detect by marker file.
                        if path.join("pyvenv.cfg").is_file() {
                            return WalkState::Skip;
                        }
                        if let Some(ext) = package_ext(path) {
                            // Opaque bundle: one item, don't descend.
                            let kind = Kind::from_ext(ext);
                            let _ = tx.send(Ok(make_item(kind, path, &entry, flags)));
                            return WalkState::Skip;
                        }
                        // A repo root is a project folder people look for; only its
                        // contents are flagged as source-tree noise.
                        if path.join(".git").exists() {
                            repos.write().unwrap().insert(path.to_path_buf());
                        }
                        let _ = tx.send(Ok(make_item(Kind::Folder, path, &entry, flags)));
                        return WalkState::Continue;
                    }
                    let kind = Kind::from_filename(&name);
                    let _ = tx.send(Ok(make_item(kind, path, &entry, flags)));
                    WalkState::Continue
                })
            });
        });
        for msg in rx {
            match msg {
                Ok(item) => {
                    if item.kind == Kind::Folder {
                        stats.folders += 1;
                    } else {
                        stats.files += 1;
                    }
                    on_item(item);
                }
                Err(()) => stats.errors += 1,
            }
        }
    });
    stats
}

fn package_ext(p: &Path) -> Option<&'static str> {
    let ext = p.extension()?.to_str()?;
    PACKAGE_EXTS
        .iter()
        .find(|e| e.eq_ignore_ascii_case(ext))
        .copied()
}

fn in_repo(path: &Path, repos: &RwLock<HashSet<PathBuf>>) -> bool {
    let repos = repos.read().unwrap();
    if repos.is_empty() {
        return false;
    }
    path.ancestors().skip(1).any(|a| repos.contains(a))
}

fn make_item(kind: Kind, path: &Path, entry: &ignore::DirEntry, flags: u32) -> Item {
    let name = if kind == Kind::App {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        entry.file_name().to_string_lossy().into_owned()
    };
    let mut item = Item::new(kind, name, path);
    item.flags = flags;
    if let Ok(md) = entry.metadata() {
        item.size = md.len();
        item.modified = mtime(&md);
    }
    if kind == Kind::App {
        if let Some(app) = crate::providers::apps::app_item(path) {
            item.alt = app.alt;
            item.bundle_id = app.bundle_id;
        }
    }
    item
}
