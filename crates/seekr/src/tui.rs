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
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use seekr_core::actions::{self, Action};
use seekr_core::{Config, Hit, Index, Kind, Query, Searcher};
use std::collections::HashSet;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RESULT_LIMIT: usize = 200;
/// Below this many columns the details pane is hidden.
const COMPACT_WIDTH: u16 = 90;
const SCOPE_CYCLE: [Option<Kind>; 4] = [None, Some(Kind::App), Some(Kind::Doc), Some(Kind::Folder)];

struct App {
    cfg: Config,
    idx: Index,
    searcher: Searcher,
    input: String,
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
    /// Set by SIGUSR1 when the launcher reveals the window.
    wake: Arc<AtomicBool>,
    /// Set by SIGTERM/SIGHUP so a resident process still restores the terminal and cleans up.
    terminate: Arc<AtomicBool>,
    last_refresh: Instant,
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
        wake: Arc::new(AtomicBool::new(false)),
        terminate: Arc::new(AtomicBool::new(false)),
        last_refresh: Instant::now(),
    };
    app.set_input(initial);
    if resident {
        use signal_hook::consts::{SIGHUP, SIGTERM, SIGUSR1};
        signal_hook::flag::register(SIGUSR1, app.wake.clone())?;
        signal_hook::flag::register(SIGTERM, app.terminate.clone())?;
        signal_hook::flag::register(SIGHUP, app.terminate.clone())?;
        launcher::write_state();
    }

    let mut terminal = ratatui::init();
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
        self.input = s;
        let q = Query::parse(&self.input);
        if let Some(k) = q.scope {
            self.ensure_kind_loaded(k);
        }
        self.searcher.set_query(q);
        self.selected = 0;
        self.dirty = true;
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
        if self.hits.is_empty() {
            self.selected = 0;
            self.list_state.select(None);
        } else {
            self.selected = self.selected.min(self.hits.len() - 1);
            self.list_state.select(Some(self.selected));
        }
        self.dirty = false;
    }

    fn flash(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    /// Done with this interaction: quit, or in resident mode reset and hide.
    fn dismiss(&mut self) {
        if !self.resident {
            self.quit = true;
            return;
        }
        self.set_input(String::new());
        self.status = None;
        self.hidden = true;
        self.final_draw = true;
        if !launcher::hide_window() {
            // No Hammerspoon to park us: behave like a normal one-shot run.
            self.quit = true;
        }
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

    fn selected_hit(&self) -> Option<&Hit> {
        self.hits.get(self.selected)
    }

    fn move_sel(&mut self, delta: isize) {
        if self.hits.is_empty() {
            return;
        }
        let n = self.hits.len() as isize;
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
            self.dismiss();
        } else {
            self.flash(format!("{} ✓", action.label()));
        }
        Ok(())
    }

    fn clear_input(&mut self) {
        self.set_input(String::new());
    }

    /// Delete the last word (`^w`, `⌥⌫`). A trailing scope prefix like `doc:` counts as a word.
    fn delete_word(&mut self) {
        let trimmed = self.input.trim_end();
        let cut = trimmed
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        let new = self.input[..cut].to_string();
        self.set_input(new);
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
        match (code, ctrl) {
            (KeyCode::Char('c'), true) | (KeyCode::Char('g'), true) => self.quit = true,
            // Spotlight convention: esc clears the query first, a second esc dismisses.
            (KeyCode::Esc, _) => {
                if self.input.is_empty() {
                    self.dismiss();
                } else {
                    self.clear_input();
                    self.flash("cleared · esc again to quit");
                }
            }
            (KeyCode::Enter, _) => self.act(Action::Open, true)?,
            (KeyCode::Char('r'), true) | (KeyCode::Char('o'), true) => {
                self.act(Action::Reveal, true)?
            }
            (KeyCode::Char('e'), true) => self.act(Action::OpenParent, true)?,
            (KeyCode::Char('y'), true) => self.act(Action::CopyPath, false)?,
            (KeyCode::Char('v'), true) => self.act(Action::QuickLook, false)?,
            (KeyCode::Char('x'), true) => self.refresh_index(true),
            (KeyCode::Tab, _) => self.cycle_scope(),
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
                if !self.hits.is_empty() {
                    self.selected = self.hits.len() - 1;
                    self.list_state.select(Some(self.selected));
                }
            }
            (KeyCode::Char('u'), true) => self.clear_input(),
            (KeyCode::Char('w'), true) => self.delete_word(),
            (KeyCode::Backspace, _) if mods.contains(KeyModifiers::ALT) => self.delete_word(),
            (KeyCode::Backspace, _) => {
                let mut s = self.input.clone();
                s.pop();
                self.set_input(s);
            }
            (KeyCode::Char(ch), false) => {
                let mut s = self.input.clone();
                s.push(ch);
                self.set_input(s);
            }
            _ => {}
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
        draw_results(f, rows[1], app);
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(rows[1]);
        draw_results(f, cols[0], app);
        draw_preview(f, cols[1], app);
    }
    draw_status(f, rows[2], app);
}

fn draw_input(f: &mut Frame, area: Rect, app: &App) {
    let q = app.searcher.query();
    let filters = q.describe_filters();
    let title = if filters.is_empty() {
        String::from(" seekr ")
    } else {
        format!(" seekr · {filters} ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let area = inner;
    let prompt = Span::styled("❯ ", Style::default().fg(Color::Cyan).bold());
    let text = Span::raw(app.input.as_str());
    let hint = if app.input.is_empty() {
        Span::styled(
            "type to search · app: doc: dir: in: ext: · ⇥ cycles scope",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::raw("")
    };
    f.render_widget(Paragraph::new(Line::from(vec![prompt, text, hint])), area);
    let text_end = area.x + 2 + app.input.chars().count() as u16;
    if !app.input.is_empty() {
        let tip = "esc clear · ^w delete word ";
        let tip_w = tip.chars().count() as u16;
        if area.right() > tip_w + 3 && text_end + 3 < area.right() - tip_w {
            let tip_area = Rect::new(area.right() - tip_w, area.y, tip_w, 1);
            f.render_widget(
                Paragraph::new(Span::styled(tip, Style::default().fg(Color::DarkGray))),
                tip_area,
            );
        }
    }
    f.set_cursor_position(Position::new(text_end.min(area.right().saturating_sub(1)), area.y));
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
    if app.input.trim().is_empty() {
        draw_home(f, area, app);
        return;
    }
    if app.hits.is_empty() && !app.loading() {
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
    let items: Vec<ListItem> = app.hits.iter().map(|h| result_item(h, width)).collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 44, 52))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, area, &mut app.list_state);
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
    let hints = [
        Line::from(vec![
            Span::styled("type to find apps, documents, folders and media", dim),
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
    if let Some(h) = app.selected_hit() {
        let it = &h.item;
        let label = Style::default().fg(Color::DarkGray);
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
    spans.push(Span::styled(
        format!("  ⏎ open · ^r reveal · ^y copy · ^v look · ⇥ scope · ^x refresh · {esc}"),
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
