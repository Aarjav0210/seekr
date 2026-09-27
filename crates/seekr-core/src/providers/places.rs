//! Places: well-known macOS locations (home, Desktop, Downloads, /Applications,
//! iCloud Drive, …) and the configured roots, each as a single folder item.
//!
//! The file walk only indexes what is *inside* the roots; this is how the
//! folders themselves become findable. Places are never walked into.
use crate::config::Config;
use crate::item::{expand_tilde, Item, Kind, FLAG_PLACE};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Standard locations and the name Finder shows for them, when that is not
/// the folder's own name. `~` is the home folder, named after the user.
const WELL_KNOWN: &[(&str, Option<&str>)] = &[
    ("~", None),
    ("~/Desktop", None),
    ("~/Documents", None),
    ("~/Downloads", None),
    ("~/Pictures", None),
    ("~/Movies", None),
    ("~/Music", None),
    ("~/Public", None),
    ("~/Applications", None),
    ("~/Library", None),
    ("~/Library/Mobile Documents/com~apple~CloudDocs", Some("iCloud Drive")),
    ("~/.Trash", Some("Trash")),
    ("/Applications", None),
    ("/Applications/Utilities", None),
];

/// Alternate name of the home folder, so `home` finds it too.
const HOME_ALT: &str = "Home";

/// Every place that exists, well-known locations first, then the roots.
pub fn scan(cfg: &Config) -> Vec<Item> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    scan_in(&home, &root_candidates(cfg))
}

/// Every path that is a place when it exists. A cached place at one of these
/// paths that no longer exists should leave the index.
pub fn candidates(cfg: &Config) -> Vec<PathBuf> {
    let home = dirs::home_dir();
    WELL_KNOWN
        .iter()
        .filter_map(|(p, _)| resolve(p, home.as_deref()))
        .chain(root_candidates(cfg))
        .collect()
}

fn root_candidates(cfg: &Config) -> Vec<PathBuf> {
    cfg.roots.iter().map(|r| expand_tilde(r)).collect()
}

fn scan_in(home: &Path, roots: &[PathBuf]) -> Vec<Item> {
    let mut seen = HashSet::new();
    let well_known = WELL_KNOWN
        .iter()
        .filter_map(|&(p, name)| Some((resolve(p, Some(home))?, name)));
    let roots = roots.iter().map(|r| (r.clone(), None));
    well_known
        .chain(roots)
        .filter(|(p, _)| seen.insert(p.clone()))
        .filter_map(|(p, name)| place_item(&p, name, home))
        .collect()
}

/// Expand a leading `~` against `home`; `None` if it needs a home we don't have.
fn resolve(p: &str, home: Option<&Path>) -> Option<PathBuf> {
    match p.strip_prefix('~') {
        Some(rest) => {
            let home = home?;
            let rest = rest.trim_start_matches('/');
            Some(if rest.is_empty() { home.to_path_buf() } else { home.join(rest) })
        }
        None => Some(PathBuf::from(p)),
    }
}

fn place_item(path: &Path, name: Option<&str>, home: &Path) -> Option<Item> {
    let md = std::fs::metadata(path).ok().filter(|m| m.is_dir())?;
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    let mut item = Item::new(Kind::Folder, name.map_or(file_name, str::to_string), path);
    if path == home {
        item.alt = Some(HOME_ALT.to_string());
    }
    item.flags = FLAG_PLACE;
    item.size = md.len();
    item.modified = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_dir;

    fn by_path<'a>(items: &'a [Item], p: &Path) -> Option<&'a Item> {
        items.iter().find(|i| i.path == p)
    }

    #[test]
    fn existing_places_are_single_folder_items() {
        let tmp = temp_dir("places");
        let home = tmp.join("jdoe");
        let icloud = home.join("Library/Mobile Documents/com~apple~CloudDocs");
        for d in [home.join("Downloads"), home.join("Desktop"), icloud.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        let root = tmp.join("Projects");
        std::fs::create_dir_all(&root).unwrap();
        let roots = [home.join("Downloads"), root.clone(), tmp.join("missing")];
        let items = scan_in(&home, &roots);

        for it in &items {
            assert_eq!(it.kind, Kind::Folder);
            assert!(it.is_place(), "{it:?}");
        }
        let h = by_path(&items, &home).unwrap();
        assert_eq!(h.name, "jdoe");
        assert_eq!(h.alt.as_deref(), Some("Home"));
        assert_eq!(by_path(&items, &home.join("Downloads")).unwrap().name, "Downloads");
        assert_eq!(by_path(&items, &icloud).unwrap().name, "iCloud Drive");
        assert_eq!(by_path(&items, &root).unwrap().name, "Projects");
        // Missing places are skipped, and a root that is also well-known appears once.
        assert!(by_path(&items, &home.join("Movies")).is_none());
        assert!(by_path(&items, &tmp.join("missing")).is_none());
        let downloads = items.iter().filter(|i| i.path == home.join("Downloads")).count();
        assert_eq!(downloads, 1);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn home_resolves_without_a_trailing_separator() {
        let home = Path::new("/Users/jdoe");
        assert_eq!(resolve("~", Some(home)).unwrap(), home);
        assert_eq!(resolve("~/Desktop", Some(home)).unwrap(), home.join("Desktop"));
        assert_eq!(resolve("/Applications", None).unwrap(), Path::new("/Applications"));
        assert!(resolve("~/Desktop", None).is_none());
    }
}
