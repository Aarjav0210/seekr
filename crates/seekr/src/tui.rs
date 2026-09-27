//! Interactive fuzzy finder.
use crate::launcher;
use crate::loader::{self, LoadOpts, Progress, Seen};
use crate::util::{ellipsize, human_size, relative_time};
use anyhow::{bail, Result};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::SetTitle;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap};
use ratatui::Frame;
use seekr_core::actions::{self, Action};
use seekr_core::calc::{self, Calculation};
use seekr_core::define::{self, Definition, Sense};
use seekr_core::{AppSearch, Config, Hit, Index, Item, Kind, Query, Searcher};
use std::collections::HashSet;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const RESULT_LIMIT: usize = 200;
/// Below this many columns the details pane is hidden.
const COMPACT_WIDTH: u16 = 90;
const SCOPE_CYCLE: [Option<Kind>; 4] = [None, Some(Kind::App), Some(Kind::Doc), Some(Kind::Folder)];
/// Nerd Font magnifier, for the row that runs an app search.
const SEARCH_GLYPH: &str = "\u{f002}";

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

struct App {
    cfg: Config,
    idx: Index,
    searcher: Searcher,
    input: String,
    /// Caret position in `input`, in chars.
    cursor: usize,
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
    /// Resident mode: Enter/Esc hide the window (via Hammerspoon) instead of quitting.
    resident: bool,
    /// Parked by the launcher; nothing is drawn until the next wake-up.
    hidden: bool,
    /// Draw one more frame after dismissing, so the window shows a clean slate when revealed.
    final_draw: bool,
    /// The whole query is selected (after a hotkey hide with text): the next
    /// character replaces it, any other key keeps it.
    input_selected: bool,
    /// Set by SIGUSR1 when the launcher reveals the window.
    wake: Arc<AtomicBool>,
    /// Set by SIGUSR2 when the launcher hid the window itself (hotkey toggle).
    park: Arc<AtomicBool>,
    /// Set by SIGTERM/SIGHUP so a resident process still restores the terminal and cleans up.
    terminate: Arc<AtomicBool>,
    last_refresh: Instant,
    /// Set while the query starts with an app-search keyword (`arc: …`).
    app_search: Option<AppSearchMode>,
    /// The query is a calculation: its answer is pinned above the hits, as row 0.
    calc: Option<Calculation>,
    /// Dictionary entry for a one-word query, looked up in the background.
    dict: DictLookup,
}

/// Skip the wake-up refresh if one ran this recently.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(5);

pub fn run(initial: String, resident: bool) -> Result<()> {
    if !std::io::stdout().is_terminal() {
        bail!("seekr's interactive UI needs a terminal; try `seekr search <query>`");
    }
    let cfg = Config::load()?;
    let idx = Index::open_default()?;
    let recent = idx
        .recent_items(100)?
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    let searcher = Searcher::new(idx.load_usage()?, recent);

    let seen: Seen = Arc::new(Mutex::new(HashSet::new()));
    let eager: Vec<Kind> = Kind::ALL
        .iter()
        .copied()
        .filter(|k| !cfg.is_lazy(*k))
        .collect();
    let first = loader::spawn(
        cfg.clone(),
        searcher.injector(),
        seen.clone(),
        LoadOpts {
            kinds: eager,
            refresh: cfg.refresh_on_launch,
        },
    );
    let last_indexed = idx.last_indexed();

    let mut app = App {
        cfg,
        idx,
        searcher,
        input: String::new(),
        cursor: 0,
        selected: 0,
        hits: Vec::new(),
        list_state: ListState::default(),
        loaders: vec![first],
        seen,
        lazy_loaded: HashSet::new(),
        status: None,
        dirty: true,
        quit: false,
        last_indexed,
        resident,
        hidden: false,
        final_draw: false,
        input_selected: false,
        wake: Arc::new(AtomicBool::new(false)),
        park: Arc::new(AtomicBool::new(false)),
        terminate: Arc::new(AtomicBool::new(false)),
        last_refresh: Instant::now(),
        app_search: None,
        calc: None,
        dict: DictLookup::spawn(),
    };
    app.set_input(initial);
    if resident {
        use signal_hook::consts::{SIGHUP, SIGTERM, SIGUSR1, SIGUSR2};
        signal_hook::flag::register(SIGUSR1, app.wake.clone())?;
        signal_hook::flag::register(SIGUSR2, app.park.clone())?;
        signal_hook::flag::register(SIGTERM, app.terminate.clone())?;
        signal_hook::flag::register(SIGHUP, app.terminate.clone())?;
        launcher::write_state();
    }

    let mut terminal = ratatui::init();
    if resident {
        // The launcher tells this window from others by its title.
        let _ = execute!(std::io::stdout(), SetTitle("seekr"));
    }
    // Own the mouse: otherwise the wheel scrolls the terminal's own buffer and
    // the UI slides out of view. Wheel events move the selection instead.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let res = event_loop(&mut terminal, &mut app);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    if resident {
        launcher::clear_state();
    }
    res
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut was_running = false;
    while !app.quit {
        if app.terminate.load(Ordering::Acquire) {
            break;
        }
        if app.park.swap(false, Ordering::AcqRel) {
            app.parked();
        }
        if app.wake.swap(false, Ordering::AcqRel) {
            app.wake_up();
        }
        if !app.hidden || app.final_draw {
            terminal.draw(|f| draw(f, app))?;
            app.final_draw = false;
        }
        // Parked windows get no input; idle slowly to stay off the CPU.
        let wait = if app.hidden { 50 } else { 16 };
        if event::poll(Duration::from_millis(wait))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    // A key while "hidden" means the window is in fact visible.
                    if app.hidden {
                        app.wake_up();
                    }
                    app.handle_key(k.code, k.modifiers)?;
                }
                Event::Mouse(m) => match m.kind {
                    MouseEventKind::ScrollDown => app.move_sel(1),
                    MouseEventKind::ScrollUp => app.move_sel(-1),
                    _ => {}
                },
                _ => {}
            }
        }
        let st = app.searcher.tick(10);
        if st.changed || app.dirty || (was_running && !st.running) {
            app.refresh_results();
        }
        was_running = st.running;
        app.announce_refreshes();
        app.dict.poll();
        if let Some((_, t)) = &app.status {
            if t.elapsed() > Duration::from_secs(3) {
                app.status = None;
            }
        }
    }
    Ok(())
}

impl App {
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
            .map(|p| p.added.load(Ordering::Relaxed))
            .sum()
    }

    fn loader_error(&self) -> Option<String> {
        self.loaders.iter().find_map(|p| p.error())
    }

    /// Show each finished refresh's report once.
    fn announce_refreshes(&mut self) {
        let reports: Vec<_> = self
            .loaders
            .iter()
            .filter_map(|p| p.take_report_once())
            .collect();
        for r in reports {
            self.last_indexed = Some(seekr_core::now_secs());
            self.flash(r.summary());
        }
        // Drop finished loaders that have nothing left to tell us.
        if self.loaders.len() > 1 {
            self.loaders.retain(|p| !p.is_done() || p.error().is_some());
        }
    }

    fn set_input(&mut self, s: String) {
        self.cursor = s.chars().count();
        self.input = s;
        self.input_selected = false;
        self.selected = 0;
        self.dirty = true;
        self.calc = calc::evaluate(&self.input).ok();
        let mut q = Query::parse_with(&self.input, &self.cfg.searches);
        self.dict.want(define::word_for_query(&q));
        if let Some(target) = q.app_search.take() {
            // Nothing from the index is shown in this mode; the matcher keeps its last query.
            let app = self.app_search.take().and_then(|m| m.app);
            self.app_search = Some(AppSearchMode {
                target,
                terms: q.terms,
                app,
            });
            return;
        }
        self.app_search = None;
        if let Some(k) = q.scope {
            self.ensure_kind_loaded(k);
        }
        self.searcher.set_query(q);
    }

    fn ensure_kind_loaded(&mut self, k: Kind) {
        if !self.cfg.is_lazy(k) || self.lazy_loaded.contains(&k) {
            return;
        }
        self.lazy_loaded.insert(k);
        let p = loader::spawn(
            self.cfg.clone(),
            self.searcher.injector(),
            self.seen.clone(),
            LoadOpts {
                kinds: vec![k],
                refresh: false,
            },
        );
        self.loaders.push(p);
        self.flash(format!("loading {} items…", k.as_str()));
    }

    fn refresh_results(&mut self) {
        self.hits = self.searcher.results(RESULT_LIMIT);
        let rows = self.row_count();
        if rows == 0 {
            self.selected = 0;
            self.list_state.select(None);
        } else {
            self.selected = self.selected.min(rows - 1);
            self.list_state.select(Some(self.selected));
        }
        self.dirty = false;
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    /// Done with this interaction: quit, or in resident mode reset and hide.
    /// `restore_focus` sends focus back to the app that was in front before
    /// seekr (Esc); after opening something, the opened app keeps it instead.
    fn dismiss(&mut self, restore_focus: bool) {
        if !self.resident {
            self.quit = true;
            return;
        }
        self.set_input(String::new());
        self.status = None;
        self.hidden = true;
        self.final_draw = true;
        if !launcher::hide_window(restore_focus) {
            // No Hammerspoon to park us: behave like a normal one-shot run.
            self.quit = true;
        }
    }

    /// The launcher hid the window without asking (the hotkey while we had focus):
    /// stop drawing, keep the query, and select it so the next reveal can either
    /// type over it or carry on with it.
    fn parked(&mut self) {
        if !self.resident {
            return;
        }
        self.hidden = true;
        self.final_draw = true;
        self.status = None;
        self.input_selected = !self.input.is_empty();
    }

    /// The launcher revealed the window: draw again and pick up new files in the background.
    fn wake_up(&mut self) {
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

    fn move_sel(&mut self, delta: isize) {
        if self.row_count() == 0 {
            return;
        }
        let n = self.row_count() as isize;
        let cur = self.selected as isize;
        self.selected = (cur + delta).rem_euclid(n) as usize;
        self.list_state.select(Some(self.selected));
    }

    fn act(&mut self, action: Action, quit_after: bool) -> Result<()> {
        let Some(hit) = self.selected_hit().cloned() else {
            self.flash("nothing selected");
            return Ok(());
        };
        match actions::perform(action, &hit.item, Some(&self.idx)) {
            Ok(Some(usage)) => {
                self.searcher.record_use(&hit.item, usage);
            }
            Ok(None) => {}
            Err(e) => {
                self.flash(format!("{}: {e}", action.label()));
                return Ok(());
            }
        }
        if quit_after {
            // The opened app has the focus now; the launcher must not hand it back.
            self.dismiss(false);
        } else {
            self.flash(format!("{} ✓", action.label()));
        }
        Ok(())
    }

    /// ⏎ or ^y on the answer row: copy the answer as a plain number. ⏎ in the
    /// launcher also hides it and hands focus back (nothing was opened); a
    /// finder in a terminal stays, with the answer still on screen.
    fn copy_calc(&mut self, dismiss: bool) {
        let Some(raw) = self.selected_calc().map(|c| c.raw.clone()) else {
            return;
        };
        if let Err(e) = actions::copy_to_clipboard(&raw) {
            self.flash(format!("Copy: {e}"));
            return;
        }
        if dismiss && self.resident {
            self.dismiss(true);
        } else {
            self.flash(format!("copied {raw} ✓"));
        }
    }

    fn clear_input(&mut self) {
        self.set_input(String::new());
    }

    /// Byte offset of char position `c` in the query.
    fn byte_at(&self, c: usize) -> usize {
        self.input.char_indices().nth(c).map_or(self.input.len(), |(i, _)| i)
    }

    /// Chars at the start of the query the caret can't enter: the `arc: `
    /// keyword that the app-search chip stands for.
    fn locked_prefix(&self) -> usize {
        match &self.app_search {
            Some(m) if self.input.ends_with(m.terms.as_str()) => {
                self.input.chars().count() - m.terms.chars().count()
            }
            _ => 0,
        }
    }

    fn move_cursor(&mut self, to: usize) {
        self.cursor = to.clamp(self.locked_prefix(), self.input.chars().count());
    }

    /// Replace chars `from..to` with `text` and leave the caret after it.
    fn splice(&mut self, from: usize, to: usize, text: &str) {
        let (a, b) = (self.byte_at(from), self.byte_at(to));
        let s = format!("{}{text}{}", &self.input[..a], &self.input[b..]);
        self.set_input(s);
        self.move_cursor(from + text.chars().count());
    }

    /// Where the word before the caret starts (`⌥←`, `^w`): skip spaces, then non-spaces.
    fn word_start(&self) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i.max(self.locked_prefix())
    }

    /// Where the word after the caret ends (`⌥→`).
    fn word_end(&self) -> usize {
        let chars: Vec<char> = self.input.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// Delete the word before the caret (`^w`, `⌥⌫`). A scope prefix like `doc:` counts as a word.
    fn delete_word(&mut self) {
        let start = self.word_start();
        self.splice(start, self.cursor, "");
    }

    /// Caret keys, shared by the index and app-search modes. False: not one of them.
    fn edit_key(&mut self, code: KeyCode, mods: KeyModifiers) -> bool {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let alt = mods.contains(KeyModifiers::ALT);
        // ⌘ arrows arrive as SUPER (contrib/kitty/seekr.conf maps them).
        let cmd = mods.contains(KeyModifiers::SUPER);
        let (lo, hi) = (self.locked_prefix(), self.input.chars().count());
        match code {
            KeyCode::Left if cmd => self.move_cursor(lo),
            KeyCode::Right if cmd => self.move_cursor(hi),
            KeyCode::Left if alt || ctrl => self.move_cursor(self.word_start()),
            KeyCode::Right if alt || ctrl => self.move_cursor(self.word_end()),
            KeyCode::Char('b') if alt => self.move_cursor(self.word_start()),
            KeyCode::Char('f') if alt => self.move_cursor(self.word_end()),
            KeyCode::Left => self.move_cursor(self.cursor.saturating_sub(1)),
            KeyCode::Right => self.move_cursor(self.cursor + 1),
            KeyCode::Char('a') if ctrl => self.move_cursor(lo),
            KeyCode::Char('u') if ctrl => self.splice(lo, self.cursor, ""),
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Backspace if alt => self.delete_word(),
            KeyCode::Backspace if cmd => self.splice(lo, self.cursor, ""),
            KeyCode::Backspace if self.cursor > lo => self.splice(self.cursor - 1, self.cursor, ""),
            KeyCode::Delete if self.cursor < hi => self.splice(self.cursor, self.cursor + 1, ""),
            KeyCode::Char(ch) if !ctrl && !alt => self.splice(self.cursor, self.cursor, &ch.to_string()),
            _ => return false,
        }
        true
    }

    /// ⇥: hand the query to the selected app's search (or to the search whose
    /// keyword was typed); with neither, cycle the scope as before.
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

    /// Keys that mean something else while an app search is being typed.
    /// Returns false to fall through to the usual handling (typing, esc, ^w, ^u, ^c).
    fn handle_search_key(&mut self, code: KeyCode, mods: KeyModifiers) -> bool {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let Some(mode) = &self.app_search else {
            return false;
        };
        let nothing_typed = mode.terms.is_empty();
        let keyword = mode.target.keyword.clone();
        let url = mode.target.url_for(&mode.terms);
        match (code, ctrl) {
            (KeyCode::Enter, _) => self.run_app_search(),
            // Nothing typed yet: ⌫ steps out of the search and leaves the keyword.
            (KeyCode::Backspace, _) if nothing_typed => self.set_input(keyword),
            // At the start of the terms, ⌫ has nothing it may delete.
            (KeyCode::Backspace, _) if self.cursor <= self.locked_prefix() => {}
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
        // The app now shows the search and has the focus; the launcher must not hand it back.
        self.dismiss(false);
    }

    fn cycle_scope(&mut self) {
        let q = Query::parse(&self.input);
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
            Some(k) => {
                if rest.is_empty() {
                    format!("{}: ", k.as_str())
                } else {
                    format!("{}: {rest}", k.as_str())
                }
            }
            None => rest.to_string(),
        };
        self.set_input(new);
    }

    /// Re-walk everything currently loaded (Ctrl-x, or silently on wake-up).
    fn refresh_index(&mut self, announce: bool) {
        if self.refreshing() {
            if announce {
                self.flash("a refresh is already running");
            }
            return;
        }
        self.last_refresh = Instant::now();
        let p = loader::spawn(
            self.cfg.clone(),
            self.searcher.injector(),
            self.seen.clone(),
            LoadOpts {
                kinds: Kind::ALL
                    .iter()
                    .copied()
                    .filter(|k| !self.cfg.is_lazy(*k) || self.lazy_loaded.contains(k))
                    .collect(),
                refresh: true,
            },
        );
        self.loaders.push(p);
        if announce {
            self.flash("refreshing index…");
        }
    }

    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Result<()> {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        // Selected query (kept from a hotkey hide): a character or backspace
        // replaces it, like a selected text field; any other key deselects and
        // acts as usual, so arrows, Enter, Tab and ^-chords keep the text.
        if std::mem::take(&mut self.input_selected) {
            match (code, ctrl) {
                (KeyCode::Char(ch), false) => {
                    self.set_input(ch.to_string());
                    return Ok(());
                }
                (KeyCode::Backspace, _) if !mods.contains(KeyModifiers::ALT) => {
                    self.clear_input();
                    return Ok(());
                }
                // Like a text field: ← collapses the selection to its start.
                (KeyCode::Left, _) => {
                    self.move_cursor(0);
                    return Ok(());
                }
                _ => {}
            }
        }
        if self.app_search.is_some() && self.handle_search_key(code, mods) {
            return Ok(());
        }
        match (code, ctrl) {
            (KeyCode::Char('c'), true) | (KeyCode::Char('g'), true) => self.quit = true,
            // Spotlight convention: esc clears the query first, a second esc dismisses.
            (KeyCode::Esc, _) => {
                if self.input.is_empty() {
                    self.dismiss(true);
                } else {
                    self.clear_input();
                    self.flash("cleared · esc again to quit");
                }
            }
            (KeyCode::Enter, _) if self.selected_calc().is_some() => self.copy_calc(true),
            (KeyCode::Enter, _) => self.act(Action::Open, true)?,
            (KeyCode::Char('r'), true) | (KeyCode::Char('o'), true) => {
                self.act(Action::Reveal, true)?
            }
            (KeyCode::Char('e'), true) => self.act(Action::OpenParent, true)?,
            (KeyCode::Char('y'), true) if self.selected_calc().is_some() => self.copy_calc(false),
            (KeyCode::Char('y'), true) => self.act(Action::CopyPath, false)?,
            (KeyCode::Char('v'), true) => self.act(Action::QuickLook, false)?,
            (KeyCode::Char('x'), true) => self.refresh_index(true),
            (KeyCode::Char('l'), true) => self.open_definition(),
            (KeyCode::Tab, _) => self.tab(),
            (KeyCode::Down, _) | (KeyCode::Char('n'), true) | (KeyCode::Char('j'), true) => {
                self.move_sel(1)
            }
            (KeyCode::Up, _) | (KeyCode::Char('p'), true) | (KeyCode::Char('k'), true) => {
                self.move_sel(-1)
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('d'), true) => self.move_sel(10),
            (KeyCode::PageUp, _) => self.move_sel(-10),
            (KeyCode::Home, _) => {
                self.selected = 0;
                self.list_state.select(Some(0));
            }
            (KeyCode::End, _) => {
                if self.row_count() > 0 {
                    self.selected = self.row_count() - 1;
                    self.list_state.select(Some(self.selected));
                }
            }
            _ => {
                self.edit_key(code, mods);
            }
        }
        Ok(())
    }
}

fn kind_color(k: Kind) -> Color {
    match k {
        Kind::App => Color::Cyan,
        Kind::Doc => Color::Yellow,
        Kind::Folder => Color::Blue,
        Kind::Image => Color::Magenta,
        Kind::Video => Color::LightRed,
        Kind::Audio => Color::Green,
        Kind::Archive => Color::DarkGray,
        Kind::Code => Color::Gray,
        Kind::File => Color::White,
    }
}

/// Wordmark for the empty query, ANSI Shadow style: 40 columns, 6 rows.
const BANNER: [&str; 6] = [
    "███████╗███████╗███████╗██╗  ██╗██████╗ ",
    "██╔════╝██╔════╝██╔════╝██║ ██╔╝██╔══██╗",
    "███████╗█████╗  █████╗  █████╔╝ ██████╔╝",
    "╚════██║██╔══╝  ██╔══╝  ██╔═██╗ ██╔══██╗",
    "███████║███████╗███████╗██║  ██╗██║  ██║",
    "╚══════╝╚══════╝╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝",
];
const BANNER_WIDTH: u16 = 40;

fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_input(f, rows[0], app);

    // Compact launcher windows: give the whole width to results.
    if area.width < COMPACT_WIDTH {
        if definition_replaces_results(app) {
            draw_definition_without_hits(f, rows[1], app);
        } else {
            let (list, card) = split_for_definition(rows[1], app, 45, 5);
            draw_results(f, list, app);
            if let Some(card) = card {
                draw_definition(f, card, app);
            }
        }
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(rows[1]);
        if definition_replaces_results(app) {
            draw_definition_without_hits(f, cols[0], app);
            draw_preview(f, cols[1], app);
        } else {
            draw_results(f, cols[0], app);
            let (details, card) = split_for_definition(cols[1], app, 55, 12);
            draw_preview(f, details, app);
            if let Some(card) = card {
                draw_definition(f, card, app);
            }
        }
    }
    draw_status(f, rows[2], app);
}

fn draw_input(f: &mut Frame, area: Rect, app: &App) {
    let filters = app.searcher.query().describe_filters();
    let title = match &app.app_search {
        Some(m) => format!(" seekr · search {} ", m.target.name()),
        None if filters.is_empty() => String::from(" seekr "),
        None => format!(" seekr · {filters} "),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let area = inner;
    let dim = Style::default().fg(Color::DarkGray);
    let selected = Style::default().fg(Color::Black).bg(Color::Cyan);
    let mut spans = vec![Span::styled("❯ ", Style::default().fg(Color::Cyan).bold())];
    let mut width = 2usize;
    match &app.app_search {
        Some(m) => {
            // The keyword becomes a chip; what follows it is shown as typed.
            let chip = format!("{} ›", m.target.name());
            width += chip.chars().count() + 1 + m.terms.chars().count();
            spans.push(Span::styled(
                chip,
                if app.input_selected {
                    selected
                } else {
                    Style::default().fg(Color::Cyan).bold()
                },
            ));
            spans.push(Span::raw(" "));
            spans.push(if app.input_selected {
                Span::styled(m.terms.clone(), selected)
            } else {
                Span::raw(m.terms.clone())
            });
            if m.terms.is_empty() {
                spans.push(Span::styled(
                    format!("type to search in {}", m.target.name()),
                    dim,
                ));
            }
        }
        None => {
            width += app.input.chars().count();
            spans.push(if app.input_selected {
                Span::styled(app.input.as_str(), selected)
            } else {
                Span::raw(app.input.as_str())
            });
            if app.input.is_empty() {
                spans.push(Span::styled(
                    "type to search · app: doc: dir: in: ext: · ⇥ cycles scope · 2+2 calculates",
                    dim,
                ));
            }
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    let text_end = area.x + width as u16;
    // The caret: after the typed text, or inside it once moved with the arrows.
    let behind = app.input.chars().count().saturating_sub(app.cursor) as u16;
    let caret = if app.input_selected { text_end } else { text_end.saturating_sub(behind) };
    let tip: Option<String> = if app.input.is_empty() {
        None
    } else if app.input_selected {
        Some("type to replace · → keeps · esc clear ".into())
    } else if let Some(m) = &app.app_search {
        Some(if !m.terms.is_empty() {
            "⏎ search · ^y copy url · esc clear ".into()
        } else if m.target.app.is_some() {
            format!("⏎ opens {} · ⌫ back · esc clear ", m.target.name())
        } else {
            "type something · ⌫ back · esc clear ".into()
        })
    } else if app.selected_calc().is_some() {
        Some("⏎ copy answer · esc clear ".into())
    } else if let Some((s, _)) = app.search_for_selection() {
        Some(format!("⇥ search in {} · esc clear ", s.name()))
    } else {
        Some("esc clear · ^w delete word ".into())
    };
    if let Some(tip) = tip {
        let tip_w = tip.chars().count() as u16;
        if area.right() > tip_w + 3 && text_end + 3 < area.right() - tip_w {
            let tip_area = Rect::new(area.right() - tip_w, area.y, tip_w, 1);
            f.render_widget(Paragraph::new(Span::styled(tip, dim)), tip_area);
        }
    }
    f.set_cursor_position(Position::new(caret.min(area.right().saturating_sub(1)), area.y));
}

fn results_block(app: &App) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " {} / {} ",
            app.searcher.matched_count(),
            app.searcher.total_items()
        ))
        .border_style(Style::default().fg(Color::DarkGray))
}

fn draw_results(f: &mut Frame, area: Rect, app: &mut App) {
    if let Some(m) = &app.app_search {
        draw_app_search(f, area, m);
        return;
    }
    if app.input.trim().is_empty() {
        draw_home(f, area, app);
        return;
    }
    if app.hits.is_empty() && app.calc.is_none() && !app.loading() {
        draw_empty(f, area, app);
        return;
    }
    let block = results_block(app);
    let inner = block.inner(area);
    f.render_widget(block, area);
    draw_list(f, inner, app);
}

/// The result rows, with the selection highlighted, filling `area`.
fn draw_list(f: &mut Frame, area: Rect, app: &mut App) {
    let width = area.width.saturating_sub(2) as usize;
    let mut items: Vec<ListItem> = app.calc.iter().map(|c| calc_item(c, width)).collect();
    items.extend(app.hits.iter().map(|h| result_item(h, width)));
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 44, 52))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, area, &mut app.list_state);
}

/// The pinned answer row: `= 8,435.5714285714  (1 + 2)^10 / 7`.
fn calc_item(c: &Calculation, width: usize) -> ListItem<'static> {
    let answer = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mut spans = vec![
        Span::styled("= ", answer),
        Span::styled(c.display.clone(), answer),
    ];
    let room = width.saturating_sub(c.display.chars().count() + 2);
    if room > 8 {
        spans.push(Span::styled(
            format!("  {}", ellipsize(&c.expr, room - 2)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    ListItem::new(Line::from(spans))
}

fn result_item(h: &Hit, width: usize) -> ListItem<'static> {
    let color = kind_color(h.item.kind);
    let mut spans = vec![Span::styled(
        format!("{} ", h.item.kind.glyph()),
        Style::default().fg(color),
    )];
    let name_chars: Vec<char> = h.item.name.chars().collect();
    let mut i = 0;
    let mut idx_iter = h.indices.iter().peekable();
    let mut buf = String::new();
    let mut hl = false;
    while i < name_chars.len() {
        let is_hl = idx_iter.peek().is_some_and(|&&x| x as usize == i);
        if is_hl {
            idx_iter.next();
        }
        if is_hl != hl && !buf.is_empty() {
            spans.push(styled_name(&buf, hl, color));
            buf.clear();
        }
        hl = is_hl;
        buf.push(name_chars[i]);
        i += 1;
    }
    if !buf.is_empty() {
        spans.push(styled_name(&buf, hl, color));
    }
    let used: usize = name_chars.len() + 3;
    let room = width.saturating_sub(used);
    if room > 8 {
        let parent = ellipsize(&h.item.parent_display(), room.saturating_sub(2));
        spans.push(Span::styled(
            format!("  {parent}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    ListItem::new(Line::from(spans))
}

/// App-search mode: the one thing ⏎ will do, in place of the result list.
fn draw_app_search(f: &mut Frame, area: Rect, m: &AppSearchMode) {
    let name = m.target.name();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {name} "))
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let dim = Style::default().fg(Color::DarkGray);
    let glyph = Span::styled(format!("{SEARCH_GLYPH} "), Style::default().fg(Color::Cyan));
    let terms = m.terms.trim();
    let (head, detail) = if terms.is_empty() {
        let head = if m.target.app.is_some() {
            format!("Open {name}")
        } else {
            format!("Search {name}")
        };
        (
            Line::from(vec![glyph, Span::raw(head)]),
            Line::from(Span::styled("  type what to search for", dim)),
        )
    } else {
        let room = inner.width.saturating_sub(6) as usize;
        (
            Line::from(vec![
                glyph,
                Span::raw(format!("Search {name} for ")),
                Span::styled(format!("\"{terms}\""), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(Span::styled(
                format!("  {}", ellipsize(&m.target.url_for(terms), room)),
                dim,
            )),
        )
    };
    let list = List::new(vec![ListItem::new(vec![head, detail])])
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 44, 52))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(0));
    f.render_stateful_widget(list, inner, &mut state);
}

/// Empty query: the wordmark, a hint, and the most used items underneath.
fn draw_home(f: &mut Frame, area: Rect, app: &mut App) {
    let block = results_block(app);
    let inner = block.inner(area);
    f.render_widget(block, area);
    // Too small for the banner: behave like a plain result list.
    if inner.width < BANNER_WIDTH + 2 || inner.height < 12 {
        draw_list(f, inner, app);
        return;
    }
    let x = inner.x + (inner.width - BANNER_WIDTH) / 2;
    for (i, row) in BANNER.iter().enumerate() {
        let line = Line::from(banner_spans(row));
        f.render_widget(Paragraph::new(line), Rect::new(x, inner.y + 1 + i as u16, BANNER_WIDTH, 1));
    }
    let dim = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Cyan);
    let mut hints = vec![
        Line::from(vec![
            Span::styled("type to find apps, documents, folders and media, or ", dim),
            Span::styled("2+2", key),
        ]),
        Line::from(vec![
            Span::styled("app:", key),
            Span::styled(" ", dim),
            Span::styled("doc:", key),
            Span::styled(" ", dim),
            Span::styled("dir:", key),
            Span::styled(" scope · ", dim),
            Span::styled("in:", key),
            Span::styled("Downloads · ", dim),
            Span::styled("ext:", key),
            Span::styled("pdf · ", dim),
            Span::styled("⇥", key),
            Span::styled(" cycles scope", dim),
        ]),
    ];
    if !app.cfg.searches.is_empty() {
        let names: Vec<&str> = app.cfg.searches.iter().map(|s| s.name()).collect();
        hints.push(Line::from(vec![
            Span::styled("⇥", key),
            Span::styled(
                format!(" on {} searches inside the app", names.join(" or ")),
                dim,
            ),
        ]));
    }
    for (i, line) in hints.into_iter().enumerate() {
        let w = line.width() as u16;
        let hx = inner.x + (inner.width.saturating_sub(w)) / 2;
        f.render_widget(Paragraph::new(line), Rect::new(hx, inner.y + 8 + i as u16, w, 1));
    }
    let list_y = inner.y + 11;
    if list_y < inner.bottom() && !app.hits.is_empty() {
        let list_area = Rect::new(inner.x, list_y, inner.width, inner.bottom() - list_y);
        draw_list(f, list_area, app);
    }
}

/// Blocks in the accent colour, the drop shadow dimmed.
fn banner_spans(row: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut solid: Option<bool> = None;
    for ch in row.chars() {
        let is_solid = ch == '█';
        if solid.is_some_and(|s| s != is_solid) {
            spans.push(banner_span(&buf, solid == Some(true)));
            buf.clear();
        }
        solid = Some(is_solid);
        buf.push(ch);
    }
    if !buf.is_empty() {
        spans.push(banner_span(&buf, solid == Some(true)));
    }
    spans
}

fn banner_span(s: &str, solid: bool) -> Span<'static> {
    let color = if solid { Color::Cyan } else { Color::DarkGray };
    Span::styled(s.to_string(), Style::default().fg(color))
}

/// Zero results for a real query: say so, and offer the refresh that fixes "I just downloaded it".
fn draw_empty(f: &mut Frame, area: Rect, app: &App) {
    let now = seekr_core::now_secs();
    let last = app
        .last_indexed
        .map(|t| relative_time(t, now))
        .unwrap_or_else(|| "never".into());
    let dim = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Cyan);
    let mut lines = vec![Line::from("")];
    lines.push(Line::from(vec![
        Span::raw("  no matches for "),
        Span::styled(app.input.clone(), Style::default().add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  just added or downloaded it?  press "),
        Span::styled("^x", key),
        Span::raw(" to refresh the index"),
    ]));
    lines.push(Line::from(Span::styled(
        format!("  index last refreshed {last}"),
        dim,
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  tips  ", dim),
        Span::raw("fewer letters · drop the scope prefix · "),
        Span::styled("code:", key),
        Span::raw(" for source files"),
    ]));
    f.render_widget(Paragraph::new(lines).block(results_block(app)), area);
}

fn styled_name(s: &str, hl: bool, color: Color) -> Span<'static> {
    if hl {
        Span::styled(
            s.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
    } else {
        Span::raw(s.to_string())
    }
}

fn draw_preview(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" details ")
        .border_style(Style::default().fg(Color::DarkGray));
    let now = seekr_core::now_secs();
    let mut lines: Vec<Line> = Vec::new();
    let label = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Cyan);
    if let Some(m) = &app.app_search {
        let name = m.target.name();
        lines.push(Line::from(Span::styled(
            format!("Search {name}"),
            Style::default().fg(Color::Cyan).bold(),
        )));
        if let Some(b) = &m.target.app {
            lines.push(Line::from(vec![
                Span::styled("app    ", label),
                Span::raw(b.clone()),
            ]));
        }
        let url = if m.terms.trim().is_empty() {
            m.target.url.clone()
        } else {
            m.target.url_for(&m.terms)
        };
        lines.push(Line::from(vec![Span::styled("url    ", label), Span::raw(url)]));
        lines.push(Line::from(""));
        let enter = if m.target.app.is_some() {
            "search · with nothing typed, open the app"
        } else {
            "search"
        };
        lines.push(Line::from(vec![Span::styled("⏎ ", key), Span::raw(enter)]));
        lines.push(Line::from(vec![
            Span::styled("^y ", key),
            Span::raw("copy url  "),
            Span::styled("⌫ ", key),
            Span::raw("back when empty  "),
            Span::styled("esc ", key),
            Span::raw("clear"),
        ]));
    } else if let Some(c) = app.selected_calc() {
        lines = calc_details(c, area.width.saturating_sub(2) as usize, app.resident);
    } else if let Some(h) = app.selected_hit() {
        let it = &h.item;
        lines.push(Line::from(vec![Span::styled(
            it.name.clone(),
            Style::default().fg(kind_color(it.kind)).bold(),
        )]));
        if let Some(alt) = &it.alt {
            lines.push(Line::from(vec![
                Span::styled("also   ", label),
                Span::raw(alt.clone()),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("kind   ", label),
            Span::raw(it.kind.label()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("path   ", label),
            Span::raw(it.path_display()),
        ]));
        if it.kind != Kind::App && it.kind != Kind::Folder {
            lines.push(Line::from(vec![
                Span::styled("size   ", label),
                Span::raw(human_size(it.size)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("edited ", label),
            Span::raw(relative_time(it.modified, now)),
        ]));
        if let Some(b) = &it.bundle_id {
            lines.push(Line::from(vec![
                Span::styled("bundle ", label),
                Span::raw(b.clone()),
            ]));
        }
        lines.push(Line::from(""));
        if let Some(s) = it
            .bundle_id
            .as_deref()
            .and_then(|b| app.cfg.search_for_app(b))
        {
            lines.push(Line::from(vec![
                Span::styled("⇥ ", key),
                Span::raw(format!("search in {}", s.name())),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("⏎ ", Style::default().fg(Color::Cyan)),
            Span::raw("open    "),
            Span::styled("^r ", Style::default().fg(Color::Cyan)),
            Span::raw("reveal in Finder"),
        ]));
        lines.push(Line::from(vec![
            Span::styled("^y ", Style::default().fg(Color::Cyan)),
            Span::raw("copy path  "),
            Span::styled("^v ", Style::default().fg(Color::Cyan)),
            Span::raw("quick look"),
        ]));
        lines.push(Line::from(vec![
            Span::styled("^e ", Style::default().fg(Color::Cyan)),
            Span::raw("open folder  "),
            Span::styled("^x ", Style::default().fg(Color::Cyan)),
            Span::raw("refresh index"),
        ]));
        lines.push(Line::from(vec![
            Span::styled("esc ", Style::default().fg(Color::Cyan)),
            Span::raw(if app.resident { "clear, then hide  " } else { "clear, then quit  " }),
            Span::styled("^u ", Style::default().fg(Color::Cyan)),
            Span::raw("clear  "),
            Span::styled("^w ⌥⌫ ", Style::default().fg(Color::Cyan)),
            Span::raw("delete word"),
        ]));
        if std::env::var_os("SEEKR_DEBUG").is_some() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("score  ", label),
                Span::raw(format!("{:.1}", h.score)),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "no results",
            Style::default().fg(Color::DarkGray),
        )));
    }
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        area,
    );
}

/// Details for the answer row: the expression, the answer in large digits
/// when they fit, and what ⏎ does.
fn calc_details(c: &Calculation, width: usize, resident: bool) -> Vec<Line<'static>> {
    let label = Style::default().fg(Color::DarkGray);
    let key = Style::default().fg(Color::Cyan);
    let answer = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mut lines = vec![
        Line::from(Span::styled("Calculator", answer)),
        Line::from(Span::styled(c.expr.clone(), label)),
        Line::from(""),
    ];
    match big_digits(&c.display).filter(|rows| rows[0].chars().count() + 1 <= width) {
        Some(rows) => {
            for row in rows {
                lines.push(Line::from(Span::styled(format!(" {row}"), answer)));
            }
        }
        None => lines.push(Line::from(Span::styled(format!("= {}", c.display), answer))),
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled("value  ", label), Span::raw(c.raw.clone())]));
    lines.push(Line::from(""));
    // In the launcher ⏎ also hides the window; ^y copies and stays.
    let mut keys = vec![Span::styled("⏎ ", key), Span::raw("copy  ")];
    if resident {
        keys[1] = Span::raw("copy and hide  ");
        keys.extend([Span::styled("^y ", key), Span::raw("copy  ")]);
    }
    keys.extend([Span::styled("↓ ", key), Span::raw("results")]);
    lines.push(Line::from(keys));
    lines
}

/// `display` in three-row digits, or None if it has characters without one
/// (scientific notation). Commas hang below the line, like a comma.
fn big_digits(display: &str) -> Option<[String; 3]> {
    let mut rows: [String; 3] = Default::default();
    for (i, ch) in display.chars().enumerate() {
        let glyph: [&str; 3] = match ch {
            '0' => ["┏━┓", "┃ ┃", "┗━┛"],
            '1' => ["╺┓ ", " ┃ ", "╺┻╸"],
            '2' => ["╺━┓", "┏━┛", "┗━╸"],
            '3' => ["╺━┓", " ━┫", "╺━┛"],
            '4' => ["╻ ╻", "┗━┫", "  ╹"],
            '5' => ["┏━╸", "┗━┓", "╺━┛"],
            '6' => ["┏━╸", "┣━┓", "┗━┛"],
            '7' => ["╺━┓", "  ┃", "  ╹"],
            '8' => ["┏━┓", "┣━┫", "┗━┛"],
            '9' => ["┏━┓", "┗━┫", "╺━┛"],
            '-' => ["   ", "╺━╸", "   "],
            '.' => [" ", " ", "▪"],
            ',' => [" ", " ", "╻"],
            _ => return None,
        };
        for (row, part) in rows.iter_mut().zip(glyph) {
            if i > 0 {
                row.push(' ');
            }
            row.push_str(part);
        }
    }
    Some(rows)
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let mut spans = Vec::new();
    if let Some(err) = app.loader_error() {
        spans.push(Span::styled(
            format!(" error: {err} "),
            Style::default().fg(Color::Red),
        ));
    } else if let Some((msg, _)) = &app.status {
        spans.push(Span::styled(
            format!(" {msg} "),
            Style::default().fg(Color::Yellow),
        ));
    } else if app.refreshing() {
        let added = app.refresh_added();
        let msg = if added > 0 {
            format!(" refreshing… +{added} new ")
        } else {
            " refreshing… ".to_string()
        };
        spans.push(Span::styled(msg, Style::default().fg(Color::Yellow)));
    } else if app.loading() {
        spans.push(Span::styled(" loading… ", Style::default().fg(Color::Yellow)));
    } else {
        spans.push(Span::styled(" ready ", Style::default().fg(Color::Green)));
    }
    let esc = if app.resident { "esc clear/hide · ^c quit" } else { "esc clear/quit" };
    let keys = if app.app_search.is_some() {
        format!("  ⏎ search · ^y copy url · ⌫ back · {esc}")
    } else if app.selected_calc().is_some() {
        format!("  ⏎ copy answer · ^y copy · ↑↓ results · {esc}")
    } else {
        let dict = if app.definition().is_some() { " · ^l dictionary" } else { "" };
        format!("  ⏎ open · ^r reveal · ^y copy · ^v look · ⇥ search/scope · ^x refresh{dict} · {esc}")
    };
    spans.push(Span::styled(keys, Style::default().fg(Color::DarkGray)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---------------------------------------------------------------------------
// Dictionary card: the definition of a one-word query, like Spotlight's.

/// Wait this long for the next keystroke before looking a word up.
const DEFINE_DEBOUNCE: Duration = Duration::from_millis(100);
/// Nerd Font book, for the definition card's title.
const DICT_GLYPH: &str = "\u{f02d}";

/// Dictionary lookups for the query, on a worker thread so typing never waits
/// (a lookup takes a few ms, the first one in a process 100–250 ms).
struct DictLookup {
    tx: mpsc::Sender<String>,
    rx: mpsc::Receiver<(String, Option<Definition>)>,
    /// The word the query asks to define.
    word: Option<String>,
    /// Its entry, once the lookup has landed.
    entry: Option<Definition>,
}

impl DictLookup {
    fn spawn() -> DictLookup {
        let (tx, requests) = mpsc::channel::<String>();
        let (results, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Load the dictionary before the first word arrives.
            define::warm_up();
            while let Ok(mut word) = requests.recv() {
                // Latest query wins: wait out the typing and look up only the newest word.
                loop {
                    match requests.recv_timeout(DEFINE_DEBOUNCE) {
                        Ok(w) => word = w,
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                let entry = define::define(&word);
                if results.send((word, entry)).is_err() {
                    return;
                }
            }
        });
        DictLookup {
            tx,
            rx,
            word: None,
            entry: None,
        }
    }

    /// The query now asks for `word` (or for nothing): answer from the cache,
    /// else ask the worker.
    fn want(&mut self, word: Option<&str>) {
        if self.word.as_deref() == word {
            return;
        }
        self.word = word.map(str::to_string);
        self.entry = None;
        if let Some(w) = word {
            match define::cached(w) {
                Some(hit) => self.entry = hit,
                None => {
                    let _ = self.tx.send(w.to_string());
                }
            }
        }
    }

    /// Take finished lookups; answers for words no longer asked for are dropped.
    fn poll(&mut self) {
        while let Ok((word, entry)) = self.rx.try_recv() {
            if self.word.as_deref() == Some(word.as_str()) {
                self.entry = entry;
            }
        }
    }
}

impl App {
    /// The dictionary entry to show for the query, if any.
    fn definition(&self) -> Option<&Definition> {
        if self.app_search.is_some() || self.input.trim().is_empty() {
            return None;
        }
        self.dict.entry.as_ref()
    }

    /// `^l`: show the word in Dictionary.app.
    fn open_definition(&mut self) {
        let Some(word) = self.definition().map(|d| d.headword.clone()) else {
            self.flash("no dictionary entry for the query");
            return;
        };
        match define::open_in_dictionary(&word) {
            // Dictionary has the focus now, like after ⏎.
            Ok(()) => self.dismiss(false),
            Err(e) => self.flash(format!("Dictionary: {e}")),
        }
    }
}

/// No index hits for a word: the card takes the place of "no matches".
fn definition_replaces_results(app: &App) -> bool {
    app.definition().is_some() && app.hits.is_empty() && app.calc.is_none() && !app.loading()
}

/// Rows the card needs to show all of `d` in a card `width` columns wide.
fn definition_height(d: &Definition, width: u16) -> u16 {
    let width = width.saturating_sub(4) as usize;
    let lines = definition_lines(d, width, 3).len() + origin_lines(d, width).len() + 1;
    lines.min(u16::MAX as usize - 2) as u16 + 2
}

/// No index hits: the card where the results were, as tall as it needs, and
/// the "no matches" note below it when there is room.
fn draw_definition_without_hits(f: &mut Frame, area: Rect, app: &App) {
    let need = app.definition().map(|d| definition_height(d, area.width)).unwrap_or(0);
    if area.height < need + 8 {
        draw_definition(f, area, app);
        return;
    }
    draw_definition(f, Rect::new(area.x, area.y, area.width, need), app);
    draw_empty(f, Rect::new(area.x, area.y + need, area.width, area.height - need), app);
}

/// Split `area` into what was there and, below it, the definition card: as
/// tall as the entry needs, at most `pct`% of `area`, leaving `keep` rows.
fn split_for_definition(area: Rect, app: &App, pct: u16, keep: u16) -> (Rect, Option<Rect>) {
    let Some(d) = app.definition() else {
        return (area, None);
    };
    let cap = (area.height * pct / 100).min(area.height.saturating_sub(keep));
    let h = definition_height(d, area.width).min(cap);
    if h < 5 {
        return (area, None);
    }
    let top = Rect::new(area.x, area.y, area.width, area.height - h);
    let card = Rect::new(area.x, area.y + area.height - h, area.width, h);
    (top, Some(card))
}

fn draw_definition(f: &mut Frame, area: Rect, app: &App) {
    let Some(d) = app.definition() else {
        return;
    };
    let dim = Style::default().fg(Color::DarkGray);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {DICT_GLYPH} definition "))
        .title_bottom(
            Line::from(vec![
                Span::styled(" ^l", Style::default().fg(Color::Cyan)),
                Span::styled(" open in Dictionary ", dim),
            ])
            .right_aligned(),
        )
        .border_style(dim)
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = fit_definition(d, inner.width as usize, inner.height as usize);
    f.render_widget(Paragraph::new(lines), inner);
}

/// The most detailed rendering of `d` that fits `height` rows, with the origin
/// when there is room; cut with an ellipsis when even the plainest does not fit.
fn fit_definition(d: &Definition, width: usize, height: usize) -> Vec<Line<'static>> {
    if width < 8 || height == 0 {
        return Vec::new();
    }
    for level in (0..=3).rev() {
        let mut lines = definition_lines(d, width, level);
        if lines.len() <= height {
            let origin = origin_lines(d, width);
            if !origin.is_empty() && lines.len() + 1 + origin.len() <= height {
                lines.push(Line::from(""));
                lines.extend(origin);
            }
            return lines;
        }
    }
    let mut lines = definition_lines(d, width, 1);
    lines.truncate(height);
    if let Some(last) = lines.pop() {
        lines.push(ellipsize_line(last, width));
    }
    lines
}

/// `d` as card lines, `width` columns wide. Detail `level`: 3 = senses,
/// subsenses and an example each; 2 = subsenses without examples; 1 = senses
/// with an example; 0 = senses only.
fn definition_lines(d: &Definition, width: usize, level: u8) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let mut out = Vec::new();
    let mut head = vec![(d.headword.clone(), Style::default().add_modifier(Modifier::BOLD))];
    if let Some(p) = &d.pronunciation {
        head.push((format!("  /{p}/"), dim));
    }
    if let Some(n) = &d.note {
        head.push((format!("  {n}"), dim));
    }
    out.extend(wrap_styled(Vec::new(), &head, width, 2));
    for part in &d.parts {
        if level >= 2 {
            out.push(Line::from(""));
        }
        let mut pos = Vec::new();
        if let Some(name) = &part.name {
            pos.push((
                name.clone(),
                Style::default().fg(Color::Magenta).add_modifier(Modifier::ITALIC),
            ));
        }
        if let Some(n) = &part.note {
            let sep = if pos.is_empty() { "" } else { "  " };
            pos.push((format!("{sep}{n}"), dim));
        }
        if !pos.is_empty() {
            out.extend(wrap_styled(Vec::new(), &pos, width, 2));
        }
        let numbered = part.senses.len() > 1;
        for (i, s) in part.senses.iter().enumerate() {
            let (prefix, indent) = if numbered {
                let n = format!("{} ", i + 1);
                let w = n.len();
                (vec![Span::styled(n, Style::default().fg(Color::Cyan))], w)
            } else {
                (Vec::new(), 0)
            };
            out.extend(wrap_styled(prefix, &sense_segments(s, level >= 1), width, indent));
            if level >= 2 {
                for sub in &s.subsenses {
                    let prefix = vec![Span::raw(" ".repeat(indent)), Span::styled("• ", dim)];
                    out.extend(wrap_styled(prefix, &sense_segments(sub, level >= 3), width, indent + 2));
                }
            }
        }
    }
    out
}

fn origin_lines(d: &Definition, width: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    match &d.origin {
        Some(o) => wrap_styled(
            vec![Span::styled("origin ", dim.add_modifier(Modifier::BOLD))],
            &[(o.clone(), dim)],
            width,
            0,
        ),
        None => Vec::new(),
    }
}

/// A sense's text, then (optionally) its first example, dimmed.
fn sense_segments(s: &Sense, example: bool) -> Vec<(String, Style)> {
    let mut segs = vec![(s.text.clone(), Style::default())];
    if example {
        if let Some(e) = s.examples.first() {
            // `[with object] : she ran the bath`: the grammar label is noise here.
            let e = match e.find("] : ") {
                Some(i) if e.starts_with('[') => &e[i + 4..],
                _ => e.as_str(),
            };
            let style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
            segs.push((": ".into(), Style::default()));
            segs.push((e.to_string(), style));
        }
    }
    segs
}

/// Word-wrap styled text into lines of at most `width` columns. The first line
/// starts with `prefix`; the others are indented by `indent` columns.
fn wrap_styled(
    prefix: Vec<Span<'static>>,
    segments: &[(String, Style)],
    width: usize,
    indent: usize,
) -> Vec<Line<'static>> {
    let indent = indent.min(width / 2);
    let mut lines = Vec::new();
    let mut used: usize = prefix.iter().map(|s| s.width()).sum();
    let mut line_start = used;
    let mut cur = prefix;
    for (text, style) in segments {
        for atom in text.split_inclusive(' ') {
            let mut atom = atom.to_string();
            loop {
                let w = Span::raw(atom.trim_end()).width();
                if used + w <= width {
                    used += Span::raw(atom.as_str()).width();
                    cur.push(Span::styled(atom, *style));
                    break;
                }
                if used > line_start {
                    // Start a new line, then retry the word there.
                    lines.push(Line::from(std::mem::take(&mut cur)));
                    cur.push(Span::raw(" ".repeat(indent)));
                    used = indent;
                    line_start = indent;
                    atom = atom.trim_start().to_string();
                    continue;
                }
                // Longer than a whole line: hard-split it.
                let room = width.saturating_sub(used).max(1);
                let head: String = atom.chars().take(room).collect();
                let rest: String = atom.chars().skip(room).collect();
                cur.push(Span::styled(head, *style));
                used = width;
                if rest.is_empty() {
                    break;
                }
                atom = rest;
            }
        }
    }
    if cur.iter().any(|s| !s.content.trim().is_empty()) {
        lines.push(Line::from(cur));
    }
    lines
}

/// Cut `line` to `width` columns and end it with an ellipsis.
fn ellipsize_line(line: Line<'static>, width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = line.spans;
    let total = |spans: &Vec<Span<'static>>| spans.iter().map(|s| s.width()).sum::<usize>();
    while total(&spans) + 2 > width {
        let Some(last) = spans.pop() else {
            break;
        };
        let mut text = last.content.into_owned();
        text.pop();
        if !text.is_empty() {
            spans.push(Span::styled(text, last.style));
        }
    }
    if let Some(last) = spans.pop() {
        let text = last.content.trim_end().to_string();
        spans.push(Span::styled(text, last.style));
    }
    spans.push(Span::styled(" …", Style::default().fg(Color::DarkGray)));
    Line::from(spans)
}
