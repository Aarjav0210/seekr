//! Application discovery.
//!
//! Two strategies:
//! 1. [`scan_app_dirs`] — a fast (tens of ms) walk of the well-known application
//!    folders. Descends through wrapper folders such as `WhatsApp.localized/` and
//!    `Chrome Apps.localized/`, which is exactly the case Spotlight fumbles.
//! 2. [`scan_launch_services`] — parses `lsregister -dump` (≈4 s) to catch apps
//!    registered from unusual locations. Used by `seekr index`.
use crate::config::Config;
use crate::item::{mtime, Item, Kind};
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

/// Walk the configured application directories and return every `.app` bundle found.
pub fn scan_app_dirs(cfg: &Config) -> Vec<Item> {
    walk_app_dirs(&cfg.app_dir_paths())
}

/// Every `.app` bundle in `dirs` (folders, or `.app` paths themselves).
pub fn walk_app_dirs(dirs: &[PathBuf]) -> Vec<Item> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();
    for root in dirs {
        if is_app_bundle(root) {
            push_app(root, &mut seen, &mut out);
            continue;
        }
        let mut it = walkdir::WalkDir::new(root)
            .min_depth(1)
            .max_depth(4)
            .follow_links(false)
            .into_iter();
        while let Some(entry) = it.next() {
            let Ok(entry) = entry else { continue };
            let p = entry.path();
            if is_app_bundle(p) {
                push_app(p, &mut seen, &mut out);
                if entry.file_type().is_dir() {
                    it.skip_current_dir();
                }
                continue;
            }
            // Other folders (`*.localized` wrappers included) are searched too,
            // except hidden ones.
            if entry.file_type().is_dir() && entry.file_name().to_string_lossy().starts_with('.') {
                it.skip_current_dir();
            }
        }
    }
    out
}

/// Parse LaunchServices' registration database for `.app` paths.
pub fn scan_launch_services() -> Result<Vec<Item>> {
    let output = Command::new(LSREGISTER).arg("-dump").output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("path:") else { continue };
        let mut p = rest.trim();
        if let Some(i) = p.rfind(" (0x") {
            p = &p[..i];
        }
        if !p.ends_with(".app") || !p.starts_with('/') {
            continue;
        }
        if p.contains("/Contents/")
            || p.contains("/.Trash/")
            || p.starts_with("/private/")
            || p.starts_with("/Volumes/")
            || p.contains("/Library/")
            || p.contains("/node_modules/")
            || p.contains("/.")
        {
            continue;
        }
        let path = Path::new(p);
        if !path.exists() {
            continue;
        }
        push_app(path, &mut seen, &mut out);
    }
    Ok(out)
}

pub fn is_app_bundle(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "app")
}

fn push_app(p: &Path, seen: &mut HashSet<PathBuf>, out: &mut Vec<Item>) {
    if !seen.insert(p.to_path_buf()) {
        return;
    }
    if let Some(item) = app_item(p) {
        out.push(item);
    }
}

/// Build an [`Item`] for an application bundle, reading `Info.plist` for
/// the bundle identifier and an alternate display name.
pub fn app_item(p: &Path) -> Option<Item> {
    let stem = p.file_stem()?.to_string_lossy().into_owned();
    let mut item = Item::new(Kind::App, stem, p);
    if let Ok(md) = std::fs::metadata(p) {
        item.modified = mtime(&md);
    }
    let plist_path = p.join("Contents").join("Info.plist");
    if let Ok(plist::Value::Dictionary(dict)) = plist::Value::from_file(&plist_path) {
        item.bundle_id = dict
            .get("CFBundleIdentifier")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string());
        let display = dict
            .get("CFBundleDisplayName")
            .and_then(|v| v.as_string())
            .or_else(|| dict.get("CFBundleName").and_then(|v| v.as_string()));
        if let Some(d) = display {
            if !d.eq_ignore_ascii_case(&item.name) && !d.is_empty() {
                item.alt = Some(d.to_string());
            }
        }
    }
    Some(item)
}
