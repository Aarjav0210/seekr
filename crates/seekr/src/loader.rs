//! Streams items into a live matcher from a background thread:
//! live app scan → cached index rows → (optionally) an incremental refresh walk
//! whose newly discovered items are streamed in as they are found.
use anyhow::Result;
use seekr_core::indexer::{self, RefreshReport};
use seekr_core::providers::apps;
use seekr_core::{Config, Index, Injector, Item, Kind, Searcher};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A background loader's state, shared with the UI.
#[derive(Default)]
pub struct Progress {
    /// New (previously unindexed) items found by the refresh walk so far.
    added: AtomicUsize,
    refreshing: AtomicBool,
    done: AtomicBool,
    /// The outcome has been handed out by `take_report`.
    reported: AtomicBool,
    report: Mutex<Option<RefreshReport>>,
    error: Mutex<Option<String>>,
}

impl Progress {
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing.load(Ordering::Acquire)
    }

    pub fn added(&self) -> usize {
        self.added.load(Ordering::Relaxed)
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }

    /// Once the loader is done: its refresh report, if it refreshed. Hands
    /// the outcome out once (for a one-time notice); `None` after that.
    pub fn take_report(&self) -> Option<RefreshReport> {
        if self.is_done() && !self.reported.swap(true, Ordering::AcqRel) {
            return self.report.lock().ok().and_then(|r| r.clone());
        }
        None
    }

    /// Done, its outcome handed out, and no error to keep showing.
    pub fn is_spent(&self) -> bool {
        self.reported.load(Ordering::Acquire) && self.error().is_none()
    }
}

pub type Seen = Arc<Mutex<HashSet<PathBuf>>>;

pub struct LoadOpts {
    pub kinds: Vec<Kind>,
    /// After loading cached items, re-walk the roots and stream anything new.
    pub refresh: bool,
}

pub fn spawn(cfg: Config, inj: Injector<Item>, seen: Seen, opts: LoadOpts) -> Arc<Progress> {
    let progress = Arc::new(Progress::default());
    let p = progress.clone();
    std::thread::Builder::new()
        .name("seekr-loader".into())
        .spawn(move || {
            if let Err(e) = load(&cfg, &inj, &seen, &opts, &p) {
                *p.error.lock().unwrap() = Some(e.to_string());
            }
            p.refreshing.store(false, Ordering::Release);
            p.done.store(true, Ordering::Release);
        })
        .expect("spawn loader thread");
    progress
}

fn load(
    cfg: &Config,
    inj: &Injector<Item>,
    seen: &Seen,
    opts: &LoadOpts,
    p: &Progress,
) -> Result<()> {
    let mut idx = Index::open_default()?;
    let push = |item: Item| {
        if seen.lock().unwrap().insert(item.path.clone()) {
            Searcher::inject(inj, item);
        }
    };
    let apps = apps::scan_app_dirs(cfg);
    feed(&idx, &apps, &opts.kinds, &push)?;

    if opts.refresh {
        p.refreshing.store(true, Ordering::Release);
        let kinds = &opts.kinds;
        let report = indexer::refresh(cfg, &mut idx, &apps, |it| {
            p.added.fetch_add(1, Ordering::Relaxed);
            if kinds.contains(&it.kind) {
                push(it.clone());
            }
        })?;
        *p.report.lock().unwrap() = Some(report);
        p.refreshing.store(false, Ordering::Release);
    }
    Ok(())
}

/// Hand `push` the live app scan (when `kinds` wants apps), then the cached
/// rows of `kinds`: apps first, so they are searchable while the rest loads.
fn feed(idx: &Index, apps: &[Item], kinds: &[Kind], mut push: impl FnMut(Item)) -> Result<()> {
    if kinds.contains(&Kind::App) {
        apps.iter().cloned().for_each(&mut push);
    }
    idx.load_items(Some(kinds))?.into_iter().for_each(push);
    Ok(())
}

/// Synchronous variant for one-shot CLI use. Builds the index if it is empty,
/// refreshes it first if `fresh`, then loads everything of `kinds` into `searcher`.
pub fn load_blocking(
    cfg: &Config,
    searcher: &Searcher,
    kinds: &[Kind],
    fresh: bool,
) -> Result<Option<RefreshReport>> {
    let mut idx = Index::open_default()?;
    let mut report = None;
    let apps = apps::scan_app_dirs(cfg);
    if idx.count()? == 0 {
        eprintln!("seekr: index is empty, building it first (this happens once)…");
        let r = indexer::reindex(cfg, &mut idx, false, |_| {})?;
        eprintln!(
            "seekr: indexed {} apps, {} files, {} folders in {:.1}s",
            r.apps,
            r.files,
            r.folders,
            r.elapsed_ms as f64 / 1000.0
        );
    } else if fresh {
        report = Some(indexer::refresh(cfg, &mut idx, &apps, |_| {})?);
    }
    let mut seen = HashSet::new();
    feed(&idx, &apps, kinds, |it| {
        if seen.insert(it.path.clone()) {
            searcher.push(it);
        }
    })?;
    Ok(report)
}
