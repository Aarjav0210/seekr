//! Primitive actions on items. These are the "tools" a higher layer (TUI, CLI, AI agent) composes.
use crate::app_search::AppSearch;
use crate::index::{Index, Usage};
use crate::item::Item;
use crate::providers::settings;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Launch the app or open the file with its default handler.
    Open,
    /// Reveal in Finder.
    Reveal,
    /// Copy the absolute path to the clipboard.
    CopyPath,
    /// Open the enclosing folder in Finder.
    OpenParent,
    /// Quick Look preview.
    QuickLook,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Open => "Open",
            Action::Reveal => "Reveal in Finder",
            Action::CopyPath => "Copy path",
            Action::OpenParent => "Open enclosing folder",
            Action::QuickLook => "Quick Look",
        }
    }

    /// Whether performing this action counts as "using" the item for frecency.
    pub fn counts_as_use(self) -> bool {
        matches!(self, Action::Open | Action::Reveal | Action::OpenParent)
    }
}

pub fn open(path: &Path) -> Result<()> {
    run("open", &[path])
}

pub fn reveal(path: &Path) -> Result<()> {
    run("open", &[OsStr::new("-R"), path.as_os_str()])
}

pub fn open_parent(path: &Path) -> Result<()> {
    run("open", &[path.parent().context("no parent directory")?])
}

pub fn quick_look(path: &Path) -> Result<()> {
    // qlmanage blocks until the preview is closed; detach it.
    Command::new("qlmanage")
        .arg("-p")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning qlmanage")?;
    Ok(())
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .context("spawning pbcopy")?;
    child
        .stdin
        .as_mut()
        .context("pbcopy stdin")?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        bail!("pbcopy exited with {status}");
    }
    Ok(())
}

/// Perform `action` on `item`, recording usage in `idx` when appropriate.
///
/// A settings pane's path is the URL that opens it, which `open` understands;
/// revealing one shows System Settings itself, and it has no Quick Look.
pub fn perform(action: Action, item: &Item, idx: Option<&Index>) -> Result<Option<Usage>> {
    match (action, item.is_setting()) {
        (Action::Open, _) => open(&item.path)?,
        (Action::CopyPath, _) => copy_to_clipboard(&item.path.to_string_lossy())?,
        (Action::Reveal | Action::OpenParent, true) => reveal(Path::new(settings::SYSTEM_SETTINGS_APP))?,
        (Action::QuickLook, true) => bail!("a settings pane has no preview"),
        (Action::Reveal, false) => reveal(&item.path)?,
        (Action::OpenParent, false) => open_parent(&item.path)?,
        (Action::QuickLook, false) => quick_look(&item.path)?,
    }
    if action.counts_as_use() {
        if let Some(idx) = idx {
            return Ok(Some(idx.record_use(&item.path)?));
        }
    }
    Ok(None)
}

/// Open a URL with the app that owns `bundle_id`, or with whatever handles its scheme.
pub fn open_url(url: &str, bundle_id: Option<&str>) -> Result<()> {
    match bundle_id {
        Some(b) => run("open", &["-b", b, url]),
        None => run("open", &[url]),
    }
}

/// Launch, or bring forward, the app with this bundle identifier.
pub fn open_app(bundle_id: &str) -> Result<()> {
    run("open", &["-b", bundle_id])
}

/// Hand `terms` to an app: open its search URL, or just the app when there is
/// nothing to search for (see [`AppSearch::open_args`]).
pub fn app_search(search: &AppSearch, terms: &str) -> Result<()> {
    let args = search.open_args(terms);
    if args.is_empty() {
        bail!("nothing to search for");
    }
    run("open", &args)
}

fn run(cmd: &str, args: &[impl AsRef<OsStr>]) -> Result<()> {
    let out = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {cmd}"))?;
    if !out.status.success() {
        let args: Vec<_> = args.iter().map(|a| a.as_ref().to_string_lossy()).collect();
        bail!(
            "{cmd} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
