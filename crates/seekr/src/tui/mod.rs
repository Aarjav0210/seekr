//! Interactive fuzzy finder: the state behind the screen and the keys that
//! change it. Every binding is listed in KEYBOARD_SHORTCUTS.md. It runs in a
//! terminal ([`run`]) or in seekr.app's window (`crate::gui`), which draws the
//! same frames itself.
//!
//! - [`query_line`]: the query as a text field.
//! - [`view`]: drawing.
//! - [`definition`]: the dictionary card for one-word queries.
mod definition;
mod query_line;
mod view;

use crate::loader::{self, LoadOpts, Progress, Seen};
use anyhow::{bail, Result};
use definition::DictLookup;
use query_line::{Edit, QueryLine};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
};
pub use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::widgets::ListState;
use ratatui::Frame;
use seekr_core::actions::{self, Action};
use seekr_core::calc::{self, Calculation};
use seekr_core::define::{self, Definition};
use seekr_core::{AppSearch, Config, Hit, Index, Item, Kind, Query, Searcher};
use std::collections::HashSet;
use std::io::IsTerminal;
use std::sync::Arc;
use std::time::{Duration, Instant};

const RESULT_LIMIT: usize = 200;
const SCOPE_CYCLE: [Option<Kind>; 4] = [None, Some(Kind::App), Some(Kind::Doc), Some(Kind::Folder)];
/// Rows PageUp and PageDown move.
const PAGE: usize = 10;
/// How long a status message stays up.
const STATUS_TTL: Duration = Duration::from_secs(3);
/// Skip the wake-up refresh if one ran this recently.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(5);

/// What the finder runs in, which decides what dismissing it does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Host {
    /// A terminal: ⏎ and esc end the program.
    Terminal,
    /// seekr.app's window: ⏎ and esc hide it, and the finder waits, loaded,
    /// for the next hotkey press.
    Window,
}

/// App-search mode (`arc: rust lifetimes`): the rest of the query goes to an
/// app instead of being matched against the index.
struct AppSearchMode {
    target: AppSearch,
    /// Everything after the keyword, verbatim.
    terms: String,
    /// The app's result row when the mode was entered from it: a search
    /// through it counts as a use, like opening the app would.
    app: Option<Item>,
}

pub struct App {
    cfg: Config,
    idx: Index,
    searcher: Searcher,
    line: QueryLine,
    /// The selected row; rows pinned above the hits count (see `pinned_rows`).
    selected: usize,
    hits: Vec<Hit>,
    list_state: ListState,
    loaders: Vec<Arc<Progress>>,
    seen: Seen,
    lazy_loaded: HashSet<Kind>,
    status: Option<(String, Instant)>,
    dirty: bool,
    quit: bool,
    last_indexed: Option<i64>,
    host: Host,
    /// The window is away (dismissed, or the hotkey hid it); the next hotkey
    /// press brings it back. Never set in a terminal.
    hidden: bool,
    last_refresh: Instant,
    /// The matcher was still busy at the last tick.
    was_running: bool,
    /// Set while the query starts with an app-search keyword (`arc: …`).
    app_search: Option<AppSearchMode>,
    /// The query is a calculation: its answer is pinned above the hits, as row 0.
    calc: Option<Calculation>,
    /// Dictionary entry for a one-word query, looked up in the background.
    dict: DictLookup,
}

pub fn run(initial: String) -> Result<()> {
    if !std::io::stdout().is_terminal() {
        bail!("seekr's interactive UI needs a terminal; try `seekr search <query>`");
    }
    let mut app = App::new(Host::Terminal)?;
    app.set_input(initial);

    let mut terminal = ratatui::init();
    // Own the mouse: otherwise the wheel scrolls the terminal's own buffer and
    // the UI slides out of view. Wheel events move the selection instead.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let res = event_loop(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.quit {
        terminal.draw(|f| app.draw(f))?;
        if event::poll(Duration::from_millis(16))? {
            app.handle_event(event::read()?);
        }
        app.tick();
    }
    Ok(())
}

impl App {
    /// The finder with the user's config and index; loading starts in the background.
    pub fn new(host: Host) -> Result<App> {
        App::with(Config::load()?, Index::open_default()?, host)
    }

    fn with(cfg: Config, idx: Index, host: Host) -> Result<App> {
        let recent = idx
            .recent_items(100)?
            .into_iter()
            .map(|(i, _)| i)
            .collect();
        let searcher = Searcher::new(idx.load_usage()?, recent);
        let mut app = App {
            last_indexed: idx.last_indexed(),
            cfg,
            idx,
            searcher,
            line: QueryLine::default(),
            selected: 0,
            hits: Vec::new(),
            list_state: ListState::default(),
            loaders: Vec::new(),
            seen: Seen::default(),
            lazy_loaded: HashSet::new(),
            status: None,
            dirty: true,
            quit: false,
            host,
            hidden: false,
            last_refresh: Instant::now(),
            was_running: false,
            app_search: None,
            calc: None,
            dict: DictLookup::spawn(),
        };
        app.spawn_loader(app.cfg.eager_kinds(), app.cfg.refresh_on_launch);
        Ok(app)
    }

    pub fn draw(&mut self, f: &mut Frame) {
        view::draw(f, self);
    }

    /// The work between frames: take in what the matcher, the loaders and the
    /// dictionary lookup have produced, and let old status messages go.
    pub fn tick(&mut self) {
        let st = self.searcher.tick(10);
        if st.changed || self.dirty || (self.was_running && !st.running) {
            self.refresh_results();
        }
        self.was_running = st.running;
        self.announce_refreshes();
        self.dict.poll();
        if self.status.as_ref().is_some_and(|(_, t)| t.elapsed() > STATUS_TTL) {
            self.status = None;
        }
    }

    /// Tick until the loaders are done and the matcher is idle (or `within`
    /// has passed), then a little longer for the dictionary lookup: a finished
    /// frame without anyone typing, for seekr.app's snapshots.
    pub fn settle(&mut self, within: Duration) {
        let start = Instant::now();
        while (self.loading() || self.was_running || self.dirty) && start.elapsed() < within {
            self.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        let linger = Instant::now();
        while linger.elapsed() < Duration::from_millis(600) {
            self.tick();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Whether the window should be away: dismissed with ⏎ or esc, or parked.
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// In seekr.app's window rather than a terminal: ⏎ and esc hide instead of quitting.
    fn in_window(&self) -> bool {
        self.host == Host::Window
    }

    /// Load `kinds` into the matcher in the background; with `refresh`, then
    /// re-walk the roots and stream in whatever is new.
    fn spawn_loader(&mut self, kinds: Vec<Kind>, refresh: bool) {
        let opts = LoadOpts { kinds, refresh };
        let p = loader::spawn(self.cfg.clone(), self.searcher.injector(), self.seen.clone(), opts);
        self.loaders.push(p);
    }

    fn loading(&self) -> bool {
        self.loaders.iter().any(|p| !p.is_done())
    }

    fn refreshing(&self) -> bool {
        self.loaders.iter().any(|p| p.is_refreshing())
    }

    /// New items found by in-flight refreshes.
    fn refresh_added(&self) -> usize {
        self.loaders
            .iter()
            .filter(|p| p.is_refreshing())
            .map(|p| p.added())
            .sum()
    }

    fn loader_error(&self) -> Option<String> {
        self.loaders.iter().find_map(|p| p.error())
    }

    /// Show each finished refresh's report once.
    fn announce_refreshes(&mut self) {
        let reports: Vec<_> = self.loaders.iter().filter_map(|p| p.take_report()).collect();
        for r in reports {
            self.last_indexed = Some(seekr_core::now_secs());
            self.flash(r.summary());
        }
        // Drop loaders with nothing left to tell (errors stay on screen).
        self.loaders.retain(|p| !p.is_spent());
    }

    /// Replace the query, caret at the end.
    fn set_input(&mut self, s: String) {
        self.line.set(s);
        self.query_changed();
    }

    /// The query's text changed: rerun the calculator, the dictionary lookup
    /// and the matcher, or enter (or stay in, or leave) app-search mode.
    fn query_changed(&mut self) {
        self.selected = 0;
        self.dirty = true;
        let text = self.line.text();
        self.calc = calc::evaluate(text).ok();
        let mut q = Query::parse_with(text, &self.cfg.searches);
        self.dict.want(define::word_for_query(&q));
        if let Some(target) = q.app_search.take() {
            // The keyword turns into a chip the caret can't enter.
            let keyword = if text.ends_with(q.terms.as_str()) {
                text.chars().count() - q.terms.chars().count()
            } else {
                0
            };
            self.line.lock(keyword);
            // Nothing from the index is shown in this mode; the matcher keeps its last query.
            let app = self.app_search.take().and_then(|m| m.app);
            self.app_search = Some(AppSearchMode {
                target,
                terms: q.terms,
                app,
            });
            return;
        }
        self.line.lock(0);
        self.app_search = None;
        if let Some(k) = q.scope {
            self.ensure_kind_loaded(k);
        }
        self.searcher.set_query(q);
    }

    /// Clipboard text typed into the query. Line breaks and other control
    /// characters are dropped: a newline would otherwise act as ⏎.
    pub fn paste(&mut self, text: &str) {
        for ch in text.chars().filter(|c| !c.is_control()) {
            self.handle_key(KeyCode::Char(ch), KeyModifiers::NONE);
        }
    }

    /// Select the whole query, as after a hotkey hide: typing replaces it.
    pub fn select_query(&mut self) {
        self.line.select_all();
    }

    /// After a key: search again if it changed the query's text.
    fn edited(&mut self, edit: Edit) {
        if edit == Edit::Changed {
            self.query_changed();
        }
    }

    fn ensure_kind_loaded(&mut self, k: Kind) {
        if !self.cfg.is_lazy(k) || !self.lazy_loaded.insert(k) {
            return;
        }
        self.spawn_loader(vec![k], false);
        self.flash(format!("loading {} items…", k.as_str()));
    }

    fn refresh_results(&mut self) {
        self.hits = self.searcher.results(RESULT_LIMIT);
        self.selected = self.selected.min(self.row_count().saturating_sub(1));
        self.dirty = false;
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    /// Done with this interaction: quit, or in the window reset and hide. The
    /// window never takes the focus from the app in front, so there is none
    /// to hand back; after ⏎ the app that was opened has it.
    fn dismiss(&mut self) {
        if !self.in_window() {
            self.quit = true;
            return;
        }
        self.set_input(String::new());
        self.status = None;
        self.hidden = true;
    }

    /// The window went away without a dismissal (the hotkey again, or a click
    /// elsewhere): keep the query, and select it so the next press can either
    /// type over it or carry on with it.
    pub fn parked(&mut self) {
        if !self.in_window() {
            return;
        }
        self.hidden = true;
        self.status = None;
        self.line.select_all();
    }

    /// The window is up again: draw, and pick up new files in the background.
    pub fn wake_up(&mut self) {
        self.hidden = false;
        self.dirty = true;
        if self.cfg.refresh_on_launch
            && !self.refreshing()
            && self.last_refresh.elapsed() > REFRESH_COOLDOWN
        {
            self.refresh_index(false);
        }
    }

    /// Rows pinned above the hits: the answer, when the query is a calculation.
    /// `selected` counts them, so hit `i` is row `i + pinned_rows()`.
    fn pinned_rows(&self) -> usize {
        usize::from(self.calc.is_some())
    }

    fn row_count(&self) -> usize {
        self.pinned_rows() + self.hits.len()
    }

    fn selected_hit(&self) -> Option<&Hit> {
        self.selected
            .checked_sub(self.pinned_rows())
            .and_then(|i| self.hits.get(i))
    }

    /// The calculation, when its answer row is the selected one.
    fn selected_calc(&self) -> Option<&Calculation> {
        self.calc.as_ref().filter(|_| self.selected == 0)
    }

    /// Move the selection one row, wrapping around at either end.
    pub fn step(&mut self, delta: isize) {
        let n = self.row_count() as isize;
        if n > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        }
    }

    /// Move the selection a page, stopping at the first or last row.
    fn page(&mut self, down: bool) {
        let last = self.row_count().saturating_sub(1);
        self.selected = if down {
            (self.selected + PAGE).min(last)
        } else {
            self.selected.saturating_sub(PAGE)
        };
    }

    /// Perform `action` on the selected hit; `dismiss_after` for the actions
    /// that bring another app forward (open, reveal).
    fn act(&mut self, action: Action, dismiss_after: bool) {
        let Some(hit) = self.selected_hit().cloned() else {
            self.flash("nothing selected");
            return;
        };
        match actions::perform(action, &hit.item, Some(&self.idx)) {
            Ok(Some(usage)) => self.searcher.record_use(&hit.item, usage),
            Ok(None) => {}
            Err(e) => {
                self.flash(format!("{}: {e}", action.label()));
                return;
            }
        }
        if dismiss_after {
            self.dismiss();
        } else {
            self.flash(format!("{} ✓", action.label()));
        }
    }

    /// ⏎ or ^y on the answer row: copy the answer as a plain number. ⏎ in the
    /// window also hides it; a finder in a terminal stays, with the answer
    /// still on screen.
    fn copy_calc(&mut self, dismiss: bool) {
        let Some(raw) = self.selected_calc().map(|c| c.raw.clone()) else {
            return;
        };
        if let Err(e) = actions::copy_to_clipboard(&raw) {
            self.flash(format!("Copy: {e}"));
            return;
        }
        if dismiss && self.in_window() {
            self.dismiss();
        } else {
            self.flash(format!("copied {raw} ✓"));
        }
    }

    /// ⇥: hand the query to the selected app's search (or to the search whose
    /// keyword was typed); with neither, cycle the scope.
    fn tab(&mut self) {
        if self.app_search.is_some() {
            return;
        }
        match self.search_for_selection() {
            Some((target, app)) => {
                self.set_input(format!("{}: ", target.keyword));
                if let Some(m) = self.app_search.as_mut() {
                    m.app = app;
                }
            }
            None => self.cycle_scope(),
        }
    }

    /// The app search ⇥ would enter, with the app's own row when it is known:
    /// the selected app's search, else the one whose keyword is the query.
    fn search_for_selection(&self) -> Option<(AppSearch, Option<Item>)> {
        if let Some(h) = self.selected_hit() {
            if h.item.kind == Kind::App {
                if let Some(s) = h
                    .item
                    .bundle_id
                    .as_deref()
                    .and_then(|b| self.cfg.search_for_app(b))
                {
                    return Some((s.clone(), Some(h.item.clone())));
                }
            }
        }
        let s = self
            .cfg
            .search_by_keyword(self.searcher.query().terms.trim())?;
        let app = self
            .hits
            .iter()
            .map(|h| &h.item)
            .find(|i| i.kind == Kind::App && i.bundle_id == s.app)
            .cloned();
        Some((s.clone(), app))
    }

    /// ⇥ without an app search: none → `app:` → `doc:` → `dir:` → none,
    /// keeping the terms and filters.
    fn cycle_scope(&mut self) {
        let q = Query::parse(self.line.text());
        let pos = SCOPE_CYCLE.iter().position(|s| *s == q.scope).unwrap_or(0);
        let next = SCOPE_CYCLE[(pos + 1) % SCOPE_CYCLE.len()];
        let mut rest = q.terms.clone();
        if let Some(d) = &q.in_dir {
            rest.push_str(&format!(" in:{d}"));
        }
        if let Some(e) = &q.ext {
            rest.push_str(&format!(" ext:{e}"));
        }
        let rest = rest.trim();
        let new = match next {
            Some(k) if rest.is_empty() => format!("{}: ", k.as_str()),
            Some(k) => format!("{}: {rest}", k.as_str()),
            None => rest.to_string(),
        };
        self.set_input(new);
    }

    /// ⏎ in app-search mode: open the URL (or just the app when nothing was
    /// typed), count it as a use of the app, and dismiss.
    fn run_app_search(&mut self) {
        let Some(mode) = &self.app_search else {
            return;
        };
        let (target, terms, app) = (mode.target.clone(), mode.terms.clone(), mode.app.clone());
        if let Err(e) = actions::app_search(&target, &terms) {
            self.flash(format!("{}: {e}", target.name()));
            return;
        }
        if let Some(app) = app {
            if let Ok(usage) = self.idx.record_use(&app.path) {
                self.searcher.record_use(&app, usage);
            }
        }
        self.dismiss();
    }

    /// Re-walk everything currently loaded (^x, or silently on wake-up).
    fn refresh_index(&mut self, announce: bool) {
        if self.refreshing() {
            if announce {
                self.flash("a refresh is already running");
            }
            return;
        }
        self.last_refresh = Instant::now();
        let kinds = Kind::ALL
            .into_iter()
            .filter(|k| !self.cfg.is_lazy(*k) || self.lazy_loaded.contains(k))
            .collect();
        self.spawn_loader(kinds, true);
        if announce {
            self.flash("refreshing index…");
        }
    }

    /// The dictionary entry to show for the query, if any.
    fn definition(&self) -> Option<&Definition> {
        if self.app_search.is_some() || self.line.text().trim().is_empty() {
            return None;
        }
        self.dict.entry()
    }

    /// `^l`: show the word in Dictionary.app.
    fn open_definition(&mut self) {
        let Some(word) = self.definition().map(|d| d.headword.clone()) else {
            self.flash("no dictionary entry for the query");
            return;
        };
        match define::open_in_dictionary(&word) {
            // Dictionary has the focus now, like after ⏎.
            Ok(()) => self.dismiss(),
            Err(e) => self.flash(format!("Dictionary: {e}")),
        }
    }

    fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind == KeyEventKind::Press => self.handle_key(k.code, k.modifiers),
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollDown => self.step(1),
                MouseEventKind::ScrollUp => self.step(-1),
                _ => {}
            },
            _ => {}
        }
    }

    pub fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        // The query selected after a hotkey hide: typing replaces it, any
        // other key keeps it and acts as usual.
        if let Some(edit) = self.line.key_on_selection(code, mods) {
            self.edited(edit);
            return;
        }
        if self.app_search.is_some() && self.handle_search_key(code, mods) {
            return;
        }
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        match (code, ctrl) {
            // The window hides: quitting would leave the hotkey with nothing to show.
            (KeyCode::Char('c' | 'g'), true) if self.in_window() => self.dismiss(),
            (KeyCode::Char('c' | 'g'), true) => self.quit = true,
            // Spotlight convention: esc clears the query first, a second esc dismisses.
            (KeyCode::Esc, _) => {
                if self.line.is_empty() {
                    self.dismiss();
                } else {
                    self.set_input(String::new());
                    let then = if self.in_window() { "hide" } else { "quit" };
                    self.flash(format!("cleared · esc again to {then}"));
                }
            }
            (KeyCode::Enter, _) if self.selected_calc().is_some() => self.copy_calc(true),
            (KeyCode::Enter, _) => self.act(Action::Open, true),
            (KeyCode::Char('r' | 'o'), true) => self.act(Action::Reveal, true),
            (KeyCode::Char('e'), true) => self.act(Action::OpenParent, true),
            (KeyCode::Char('y'), true) if self.selected_calc().is_some() => self.copy_calc(false),
            (KeyCode::Char('y'), true) => self.act(Action::CopyPath, false),
            (KeyCode::Char('v'), true) => self.act(Action::QuickLook, false),
            (KeyCode::Char('x'), true) => self.refresh_index(true),
            (KeyCode::Char('l'), true) => self.open_definition(),
            (KeyCode::Tab, _) => self.tab(),
            (KeyCode::Down, _) | (KeyCode::Char('n' | 'j'), true) => self.step(1),
            (KeyCode::Up, _) | (KeyCode::Char('p' | 'k'), true) => self.step(-1),
            (KeyCode::PageDown, _) | (KeyCode::Char('d'), true) => self.page(true),
            (KeyCode::PageUp, _) => self.page(false),
            (KeyCode::Home, _) => self.selected = 0,
            (KeyCode::End, _) => self.selected = self.row_count().saturating_sub(1),
            _ => {
                let edit = self.line.key(code, mods);
                self.edited(edit);
            }
        }
    }

    /// Keys that mean something else while an app search is being typed.
    /// Returns false to fall through to the usual handling (typing, esc, ^w, ^u, ^c).
    fn handle_search_key(&mut self, code: KeyCode, mods: KeyModifiers) -> bool {
        let Some(mode) = &self.app_search else {
            return false;
        };
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let nothing_typed = mode.terms.is_empty();
        let keyword = mode.target.keyword.clone();
        let url = mode.target.url_for(&mode.terms);
        match (code, ctrl) {
            (KeyCode::Enter, _) => self.run_app_search(),
            // Nothing typed yet: ⌫ steps out of the search and leaves the keyword.
            (KeyCode::Backspace, _) if nothing_typed => self.set_input(keyword),
            (KeyCode::Char('y'), true) => match actions::copy_to_clipboard(&url) {
                Ok(()) => self.flash("Copy URL ✓"),
                Err(e) => self.flash(format!("Copy URL: {e}")),
            },
            // There is no list here: selection, scope and item actions are inert.
            (
                KeyCode::Tab
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End,
                _,
            ) => {}
            (KeyCode::Char('r' | 'o' | 'e' | 'v' | 'n' | 'p' | 'j' | 'k' | 'd'), true) => {}
            _ => return false,
        }
        true
    }
}
