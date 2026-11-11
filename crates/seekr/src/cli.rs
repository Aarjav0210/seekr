//! Non-interactive subcommands.
use crate::loader;
use crate::util::{ellipsize, human_size, relative_time};
use anyhow::{bail, Result};
use seekr_core::actions::{self, Action};
use seekr_core::define::{self, Sense};
use seekr_core::providers::{apps, settings};
use seekr_core::search::kinds_for_query;
use seekr_core::{calc, doctor, indexer, Config, Index, Item, Kind, Query, Searcher};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn search(raw: &str, limit: usize, json: bool, all: bool, fresh: bool) -> Result<()> {
    let cfg = Config::load()?;
    let idx = Index::open_default()?;
    let q = Query::parse(raw);
    let kinds = if all {
        Kind::ALL.to_vec()
    } else {
        kinds_for_query(&q, &cfg)
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
    s.settle(Duration::from_secs(20));
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
            ellipsize(&h.item.name, 40),
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
    // A settings pane's URL, as `seekr search` prints it.
    if let Some(id) = path.to_string_lossy().strip_prefix(settings::URL_PREFIX) {
        let item = settings::item(id).unwrap_or_else(|| Item::new(Kind::Setting, id, path));
        let action = if reveal { Action::Reveal } else { Action::Open };
        actions::perform(action, &item, Some(&idx))?;
        return Ok(());
    }
    // Spelled like the index spells it (absolute, no `./` or trailing `/`),
    // so the use counts for that item's ranking.
    let path: PathBuf = std::path::absolute(path)?.components().collect();
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let kind = if path.is_dir() && !apps::is_app_bundle(&path) {
        Kind::Folder
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
    let apps = apps::scan_app_dirs(&cfg);
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
            "apps: {}  (+{} from LaunchServices)\nsettings panes: {}\nplaces: {}\nfiles: {}\nfolders: {}\nwalk errors: {}\ntook: {:.2}s\ndb: {}",
            report.apps,
            report.lsregister_apps,
            report.settings,
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

/// The configured app searches, and whether their apps are installed. Those
/// whose app is missing are listed last; Tab does nothing special for them.
pub fn searches(json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let rows = cfg
        .searches
        .iter()
        .map(|s| (s, true))
        .chain(cfg.unavailable.iter().map(|s| (s, false)));
    if json {
        let v: Vec<_> = rows
            .map(|(s, installed)| {
                serde_json::json!({
                    "keyword": s.keyword,
                    "name": s.name(),
                    "url": s.url,
                    "app": s.app,
                    "installed": s.app.as_ref().map(|_| installed),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!("{:<10} {:<16} {:<14} {}", "keyword", "name", "app", "url");
    for (s, installed) in rows {
        let status = match (&s.app, installed) {
            (None, _) => "web",
            (Some(_), true) => "installed",
            (Some(_), false) => "not installed",
        };
        println!("{:<10} {:<16} {:<14} {}", s.keyword, s.name(), status, s.url);
    }
    println!("\nfinder: select the app (or type its keyword) and press ⇥; shell: seekr search-in <keyword> <terms>");
    println!("config: {} ([[searches]])", Config::config_path().display());
    Ok(())
}

/// The System Settings panes installed on this Mac, as the finder lists them.
pub fn settings(json: bool) -> Result<()> {
    let panes = settings::scan();
    if json {
        let v: Vec<_> = panes
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "words": p.alt,
                    "id": p.bundle_id,
                    "url": p.path,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    if panes.is_empty() {
        eprintln!("no System Settings panes found (seekr knows the layout of macOS 13 and later)");
        return Ok(());
    }
    let mut sorted: Vec<&Item> = panes.iter().collect();
    sorted.sort_by_key(|p| p.name.to_lowercase());
    println!("{:<26} {:<44} {}", "pane", "also found by", "opens");
    for p in sorted {
        println!(
            "{:<26} {:<44} {}",
            ellipsize(&p.name, 26),
            ellipsize(p.alt.as_deref().unwrap_or(""), 44),
            p.path_display()
        );
    }
    println!("\nfinder: type the pane's name and press ⏎; shell: seekr open <url>, or `open <url>`");
    Ok(())
}

/// `seekr search-in spotify daft punk`: hand the terms to an app, or with
/// `--print` just show what would be opened.
pub fn search_in(keyword: &str, terms: &str, print: bool, json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let Some(s) = cfg.search_by_keyword(keyword) else {
        if let Some(s) = seekr_core::app_search::by_keyword(&cfg.unavailable, keyword) {
            bail!("{} is not installed, so the {keyword:?} search is off", s.name());
        }
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

/// `seekr web what's the weather`: search the web for the terms in the
/// default browser, or with `--print` just show the URL.
pub fn web(terms: &str, print: bool, json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let s = cfg.web_search();
    let terms = terms.trim();
    if terms.is_empty() {
        bail!("nothing to search for");
    }
    if !print {
        actions::app_search(&s, terms)?;
    }
    let url = s.url_for(terms);
    if json {
        let v = serde_json::json!({
            "browser": s.name(),
            "terms": terms,
            "url": url,
            "command": ["open", &url],
            "opened": !print,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else if print {
        println!("{url}");
    }
    Ok(())
}

/// `seekr calc 2+2*3`: print the answer (`8`; with thousands separators on a
/// terminal, plain when piped), or with `--json` the whole [`calc::Calculation`].
/// Not a calculation, division by zero and undefined results exit non-zero
/// (with `--json`, after printing `{"input", "error", "message"}`).
pub fn calc(expr: &str, json: bool) -> Result<()> {
    match calc::evaluate(expr) {
        Ok(c) if json => println!("{}", serde_json::to_string_pretty(&c)?),
        Ok(c) if std::io::stdout().is_terminal() => println!("{}", c.display),
        Ok(c) => println!("{}", c.raw),
        Err(e) if json => {
            let v = serde_json::json!({ "input": expr, "error": e, "message": e.to_string() });
            println!("{}", serde_json::to_string_pretty(&v)?);
            std::process::exit(1);
        }
        Err(e) => bail!("{e}: {expr:?}"),
    }
    Ok(())
}

/// `seekr define serendipity`: the system dictionary's entry, or exit 1.
pub fn define(word: &str, json: bool) -> Result<()> {
    let word = word.trim();
    if !define::is_word(word) {
        eprintln!("seekr: {word:?} is not a single word (letters, inner - or ' only)");
        std::process::exit(1);
    }
    let Some(d) = define::define(word) else {
        if define::available() {
            eprintln!("seekr: no dictionary entry for {word:?}");
        } else {
            eprintln!(
                "seekr: the macOS dictionary is not available (it downloads on first use; try again shortly)"
            );
        }
        std::process::exit(1);
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&d)?);
        return Ok(());
    }
    match &d.pronunciation {
        Some(p) => println!("{}  /{p}/", d.headword),
        None => println!("{}", d.headword),
    }
    if let Some(n) = &d.note {
        println!("{n}");
    }
    for part in &d.parts {
        println!();
        match (&part.name, &part.note) {
            (Some(name), Some(note)) => println!("{name}  {note}"),
            (Some(name), None) => println!("{name}"),
            (None, Some(note)) => println!("{note}"),
            (None, None) => {}
        }
        let numbered = part.senses.len() > 1;
        for (i, s) in part.senses.iter().enumerate() {
            let lead = if numbered { format!("{:>2}. ", i + 1) } else { "    ".into() };
            print_sense(&lead, s);
            for sub in &s.subsenses {
                print_sense("     • ", sub);
            }
        }
    }
    if let Some(o) = &d.origin {
        println!("\norigin: {o}");
    }
    Ok(())
}

fn print_sense(lead: &str, s: &Sense) {
    match s.examples.first() {
        Some(e) => println!("{lead}{}: ‘{e}’", s.text),
        None => println!("{lead}{}", s.text),
    }
}
