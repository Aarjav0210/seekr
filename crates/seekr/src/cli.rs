//! Non-interactive subcommands.
use crate::loader;
use crate::util::{human_size, relative_time};
use anyhow::{bail, Result};
use seekr_core::actions::{self, Action};
use seekr_core::providers::apps;
use seekr_core::search::kinds_for_query;
use seekr_core::{doctor, indexer, AppSearch, Config, Index, Item, Kind, Query, Searcher};
use std::collections::HashSet;
use std::path::Path;

pub fn search(raw: &str, limit: usize, json: bool, all: bool, fresh: bool) -> Result<()> {
    let cfg = Config::load()?;
    let idx = Index::open_default()?;
    let q = Query::parse(raw);
    let lazy: Vec<Kind> = Kind::ALL
        .iter()
        .copied()
        .filter(|k| cfg.is_lazy(*k))
        .collect();
    let kinds = if all {
        Kind::ALL.to_vec()
    } else {
        kinds_for_query(&q, &lazy)
    };
    let recent = idx
        .recent_items(50)?
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    let mut s = Searcher::new(idx.load_usage()?, recent);
    if let Some(r) = loader::load_blocking(&cfg, &s, &kinds, fresh)? {
        eprintln!("seekr: {}", r.summary());
    }
    s.set_query(q);
    // Drive the matcher to completion.
    let mut spins = 0;
    loop {
        let st = s.tick(50);
        spins += 1;
        if !st.running && spins > 1 {
            break;
        }
        if spins > 400 {
            break;
        }
    }
    let hits = s.results(limit);
    if json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(());
    }
    if hits.is_empty() {
        eprintln!("no matches for {raw:?} ({} items searched)", s.total_items());
        return Ok(());
    }
    let now = seekr_core::now_secs();
    for (i, h) in hits.iter().enumerate() {
        println!(
            "{:>2}. {:<7} {:<40} {}",
            i + 1,
            h.item.kind.as_str(),
            crate::util::ellipsize(&h.item.name, 40),
            h.item.path_display()
        );
        if std::env::var_os("SEEKR_DEBUG").is_some() {
            println!(
                "    score={:.1} size={} modified={}",
                h.score,
                human_size(h.item.size),
                relative_time(h.item.modified, now)
            );
        }
    }
    eprintln!(
        "{} of {} matches shown ({} items searched)",
        hits.len(),
        s.matched_count(),
        s.total_items()
    );
    Ok(())
}

pub fn open(path: &Path, reveal: bool) -> Result<()> {
    let idx = Index::open_default()?;
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let kind = if path.is_dir() {
        if path.extension().is_some_and(|e| e == "app") {
            Kind::App
        } else {
            Kind::Folder
        }
    } else {
        Kind::from_filename(&name)
    };
    let item = Item::new(kind, name, path);
    let action = if reveal { Action::Reveal } else { Action::Open };
    actions::perform(action, &item, Some(&idx))?;
    Ok(())
}

pub fn refresh(json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let mut idx = Index::open_default()?;
    if idx.count()? == 0 {
        return index(false, json);
    }
    let apps = seekr_core::providers::apps::scan_app_dirs(&cfg);
    let report = indexer::refresh(&cfg, &mut idx, &apps, |_| {})?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{} ({} items scanned)", report.summary(), report.scanned);
    }
    Ok(())
}

pub fn index(full: bool, json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let mut idx = Index::open_default()?;
    if !json {
        eprintln!(
            "indexing {} app dirs and {} roots{}…",
            cfg.app_dir_paths().len(),
            cfg.root_paths().len(),
            if full { " (+ LaunchServices)" } else { "" }
        );
    }
    let report = indexer::reindex(&cfg, &mut idx, full, |_| {})?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "apps: {}  (+{} from LaunchServices)\nplaces: {}\nfiles: {}\nfolders: {}\nwalk errors: {}\ntook: {:.2}s\ndb: {}",
            report.apps,
            report.lsregister_apps,
            report.places,
            report.files,
            report.folders,
            report.walk_errors,
            report.elapsed_ms as f64 / 1000.0,
            idx.path().display()
        );
    }
    Ok(())
}

pub fn doctor(json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let idx = Index::open_default().ok();
    let report = doctor::run(&cfg, idx.as_ref());
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("Spotlight");
    for c in &report.spotlight {
        println!("  [{}] {:<28} {}", if c.ok { "ok" } else { "!!" }, c.name, c.detail);
    }
    println!("\nseekr index");
    match &report.index {
        Some(s) => {
            let now = seekr_core::now_secs();
            println!("  db:           {} ({})", s.db_path.display(), human_size(s.db_bytes));
            println!(
                "  last refresh: {}",
                s.last_indexed
                    .map(|t| relative_time(t, now))
                    .unwrap_or_else(|| "never".into())
            );
            println!("  items:        {}", s.total);
            for (k, n) in &s.by_kind {
                println!("    {:<8} {}", k.as_str(), n);
            }
            println!("  usage rows:   {}", s.usage_entries);
        }
        None => println!("  (could not open index)"),
    }
    println!("\nconfig: {}", report.config_path);
    println!("roots:");
    for r in &report.roots {
        println!("  {r}");
    }
    println!("app dirs:");
    for r in &report.app_dirs {
        println!("  {r}");
    }
    if !report.hints.is_empty() {
        println!("\nhints:");
        for h in &report.hints {
            println!("  • {h}");
        }
    }
    Ok(())
}

pub fn kinds(json: bool) -> Result<()> {
    if json {
        let v: Vec<_> = Kind::ALL
            .iter()
            .map(|k| serde_json::json!({ "kind": k.as_str(), "label": k.label() }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!("scope    label          example");
    for k in Kind::ALL {
        println!("{:<8} {:<14} {}: <terms>", k.as_str(), k.label(), k.as_str());
    }
    println!("\nfilters: in:<path-substring>   ext:<extension>");
    let searches = Config::load()?.searches;
    if !searches.is_empty() {
        let kws: Vec<String> = searches.iter().map(|s| format!("{}:", s.keyword)).collect();
        println!(
            "app searches: {}   (⇥ in the finder; see `seekr searches`)",
            kws.join(" ")
        );
    }
    Ok(())
}

/// The configured app searches, and whether their apps are installed.
pub fn searches(json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let installed: HashSet<String> = apps::scan_app_dirs(&cfg)
        .into_iter()
        .filter_map(|a| a.bundle_id)
        .collect();
    if json {
        let v: Vec<_> = cfg
            .searches
            .iter()
            .map(|s| {
                serde_json::json!({
                    "keyword": s.keyword,
                    "name": s.name(),
                    "url": s.url,
                    "app": s.app,
                    "installed": s.app.as_ref().map(|b| installed.contains(b)),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let status = |s: &AppSearch| match &s.app {
        Some(b) if installed.contains(b) => "installed",
        Some(_) => "not installed",
        None => "web",
    };
    println!("{:<10} {:<10} {:<14} {}", "keyword", "name", "app", "url");
    for s in &cfg.searches {
        println!("{:<10} {:<10} {:<14} {}", s.keyword, s.name(), status(s), s.url);
    }
    println!("\nfinder: select the app (or type its keyword) and press ⇥; shell: seekr search-in <keyword> <terms>");
    println!("config: {} ([[searches]])", Config::config_path().display());
    Ok(())
}

/// `seekr search-in spotify daft punk`: hand the terms to an app, or with
/// `--print` just show what would be opened.
pub fn search_in(keyword: &str, terms: &str, print: bool, json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let Some(s) = cfg.search_by_keyword(keyword) else {
        bail!("no app search named {keyword:?}; `seekr searches` lists them");
    };
    let terms = terms.trim();
    let args = s.open_args(terms);
    if args.is_empty() {
        bail!("nothing to search for");
    }
    if !print {
        actions::app_search(s, terms)?;
    }
    let url = (!terms.is_empty()).then(|| s.url_for(terms));
    if json {
        let command: Vec<String> = std::iter::once("open".to_string()).chain(args).collect();
        let v = serde_json::json!({
            "keyword": s.keyword,
            "name": s.name(),
            "app": s.app,
            "terms": terms,
            "url": url,
            "command": command,
            "opened": !print,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else if print {
        match url {
            Some(u) => println!("{u}"),
            None => println!("open {}", args.join(" ")),
        }
    }
    Ok(())
}
