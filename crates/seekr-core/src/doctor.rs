//! Diagnostics: is Spotlight healthy? Is our index healthy?
use crate::config::Config;
use crate::index::{Index, Stats};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub spotlight: Vec<Check>,
    pub index: Option<Stats>,
    pub config_path: String,
    pub roots: Vec<String>,
    pub app_dirs: Vec<String>,
    pub hints: Vec<String>,
}

pub fn run(cfg: &Config, idx: Option<&Index>) -> Report {
    let mut spotlight = Vec::new();
    let mut hints = Vec::new();

    let root = sh("mdutil", &["-s", "/"]);
    let root_ok = root.contains("Indexing enabled");
    spotlight.push(Check {
        name: "mdutil / (system volume)".into(),
        ok: root_ok,
        detail: squash(&root),
    });

    let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned());
    if let Some(h) = &home {
        let out = sh("mdutil", &["-s", h]);
        let ok = out.contains("Indexing enabled");
        if !ok {
            hints.push(
                "Spotlight reports an unknown indexing state for your home folder. \
                 To rebuild Spotlight's index: `sudo mdutil -E /` (takes a while; seekr does not need it)."
                    .into(),
            );
        }
        spotlight.push(Check {
            name: "mdutil ~ (home)".into(),
            ok,
            detail: squash(&out),
        });
    }

    // Does Spotlight actually have metadata for a file that certainly exists?
    let probe = "/System/Applications/Calculator.app";
    let out = sh("mdls", &["-name", "kMDItemContentType", probe]);
    let ok = !out.contains("(null)") && !out.is_empty();
    if !ok {
        hints.push(
            "Spotlight returns no metadata even for system apps; its index is effectively empty. \
             This is why Spotlight falls back to App Store results."
                .into(),
        );
    }
    spotlight.push(Check {
        name: "mdls probe (Calculator.app)".into(),
        ok,
        detail: squash(&out),
    });

    let index = idx.and_then(|i| i.stats().ok());
    if let Some(s) = &index {
        if s.total == 0 {
            hints.push("Index is empty. Run `seekr index` (or just launch `seekr`; it indexes on first run).".into());
        }
    }

    Report {
        spotlight,
        index,
        config_path: Config::config_path().to_string_lossy().into_owned(),
        roots: cfg
            .root_paths()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        app_dirs: cfg
            .app_dir_paths()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        hints,
    }
}

fn sh(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        })
        .unwrap_or_else(|e| format!("failed to run {cmd}: {e}"))
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
