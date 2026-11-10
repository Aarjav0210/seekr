//! Drawing: the query line, the results (or the home screen, the app search,
//! the calculator's answer), the details pane, the definition card and the
//! status bar.
use super::definition;
use super::{App, AppSearchMode};
use crate::util::{ellipsize, human_size, relative_time};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use seekr_core::calc::Calculation;
use seekr_core::{Hit, Kind};

/// Below this many columns the details pane is hidden.
const COMPACT_WIDTH: u16 = 90;
/// Nerd Font magnifier, for the row that runs an app search.
const SEARCH_GLYPH: &str = "\u{f002}";

/// Frames, labels, hints.
pub const DIM: Style = Style::new().fg(Color::DarkGray);
/// Keys in hints.
pub const KEY: Style = Style::new().fg(Color::Cyan);
/// The prompt, the app-search chip, answers.
const ACCENT: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
/// The selected row.
const HIGHLIGHT: Style = Style::new().bg(Color::Rgb(40, 44, 52)).add_modifier(Modifier::BOLD);
/// The whole query, selected after a hotkey hide.
const SELECTED_TEXT: Style = Style::new().fg(Color::Black).bg(Color::Cyan);

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

pub fn draw(f: &mut Frame, app: &mut App) {
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

/// A box with a dim frame.
pub fn pane<'a>(title: impl Into<Line<'a>>) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(DIM)
}

/// `⏎ open  ^r reveal`: each key in the key colour, then what it does (with
/// the spacing that should follow it).
fn hints(pairs: &[(&str, &str)]) -> Line<'static> {
    let spans: Vec<Span> = pairs
        .iter()
        .flat_map(|&(key, what)| [Span::styled(format!("{key} "), KEY), Span::raw(what.to_string())])
        .collect();
    Line::from(spans)
}

/// Dim text with keys in it: even `parts` are dim, odd ones keys.
fn keyed(parts: &[&str]) -> Line<'static> {
    let spans: Vec<Span> = parts
        .iter()
        .enumerate()
        .map(|(i, p)| Span::styled(p.to_string(), if i % 2 == 0 { DIM } else { KEY }))
        .collect();
    Line::from(spans)
}

/// `kind   Document`: a dim label, then the value.
fn field(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{label:<7}"), DIM), Span::raw(value.into())])
}

/// `s` cut into the longest runs of chars on which `pred(index, char)` agrees.
fn runs(s: &str, pred: impl Fn(usize, char) -> bool) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (i, ch) in s.chars().enumerate() {
        let p = pred(i, ch);
        match out.last_mut() {
            Some((run, q)) if *q == p => run.push(ch),
            _ => out.push((ch.to_string(), p)),
        }
    }
    out
}

fn draw_input(f: &mut Frame, outer: Rect, app: &App) {
    let filters = app.searcher.query().describe_filters();
    let title = match &app.app_search {
        Some(m) => format!(" seekr · search {} ", m.target.name()),
        None if filters.is_empty() => String::from(" seekr "),
        None => format!(" seekr · {filters} "),
    };
    let block = pane(title);
    let area = block.inner(outer);
    f.render_widget(block, outer);
    let line = &app.line;
    let selected = line.is_selected();
    let text_style = if selected { SELECTED_TEXT } else { Style::new() };
    let mut spans = vec![Span::styled("❯ ", ACCENT)];
    let placeholder = match &app.app_search {
        Some(m) => {
            // The keyword becomes a chip; what follows it is shown as typed.
            let chip = format!("{} ›", m.target.name());
            spans.push(Span::styled(chip, if selected { SELECTED_TEXT } else { ACCENT }));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(m.terms.as_str(), text_style));
            m.terms
                .is_empty()
                .then(|| format!("type to search in {}", m.target.name()))
        }
        None => {
            spans.push(Span::styled(line.text(), text_style));
            line.is_empty().then(|| {
                "type to search · app: doc: dir: in: ext: · ⇥ cycles scope · 2+2 calculates".to_string()
            })
        }
    };
    let text_width: usize = spans.iter().map(Span::width).sum();
    let text_end = area.x.saturating_add(text_width.min(u16::MAX as usize) as u16);
    spans.extend(placeholder.map(|p| Span::styled(p, DIM)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    // The caret: after the typed text, or inside it once moved with the arrows.
    let behind = Span::raw(line.after_cursor()).width().min(u16::MAX as usize) as u16;
    let caret = if selected { text_end } else { text_end.saturating_sub(behind) };
    let tip: Option<String> = if line.is_empty() {
        None
    } else if selected {
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
        let tip_w = Span::raw(tip.as_str()).width() as u16;
        if area.right() > tip_w + 3 && text_end.saturating_add(3) < area.right() - tip_w {
            let tip_area = Rect::new(area.right() - tip_w, area.y, tip_w, 1);
            f.render_widget(Paragraph::new(Span::styled(tip, DIM)), tip_area);
        }
    }
    f.set_cursor_position(Position::new(caret.min(area.right().saturating_sub(1)), area.y));
}

fn results_block(app: &App) -> Block<'static> {
    pane(format!(
        " {} / {} ",
        app.searcher.matched_count(),
        app.searcher.total_items()
    ))
}

fn draw_results(f: &mut Frame, area: Rect, app: &mut App) {
    if let Some(m) = &app.app_search {
        draw_app_search(f, area, m);
        return;
    }
    if app.line.text().trim().is_empty() {
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

/// A list whose selected row is highlighted.
fn selectable<'a>(items: impl IntoIterator<Item = ListItem<'a>>) -> List<'a> {
    List::new(items)
        .highlight_style(HIGHLIGHT)
        .highlight_symbol("▸ ")
}

/// The result rows, with the selection highlighted, filling `area`.
fn draw_list(f: &mut Frame, area: Rect, app: &mut App) {
    let width = area.width.saturating_sub(2) as usize;
    let mut items: Vec<ListItem> = app.calc.iter().map(|c| calc_item(c, width)).collect();
    items.extend(app.hits.iter().map(|h| result_item(h, width)));
    app.list_state.select((!items.is_empty()).then_some(app.selected));
    f.render_stateful_widget(selectable(items), area, &mut app.list_state);
}

/// The pinned answer row: `= 8,435.5714285714  (1 + 2)^10 / 7`.
fn calc_item(c: &Calculation, width: usize) -> ListItem<'static> {
    let mut spans = vec![
        Span::styled("= ", ACCENT),
        Span::styled(c.display.clone(), ACCENT),
    ];
    let room = width.saturating_sub(c.display.chars().count() + 2);
    if room > 8 {
        spans.push(Span::styled(format!("  {}", ellipsize(&c.expr, room - 2)), DIM));
    }
    ListItem::new(Line::from(spans))
}

fn result_item(h: &Hit, width: usize) -> ListItem<'static> {
    let color = kind_color(h.item.kind);
    let matched = Style::new()
        .fg(color)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut spans = vec![Span::styled(
        format!("{} ", h.item.kind.glyph()),
        Style::new().fg(color),
    )];
    // `indices` is sorted: the matched chars of the name.
    for (run, hl) in runs(&h.item.name, |i, _| h.indices.binary_search(&(i as u32)).is_ok()) {
        spans.push(if hl { Span::styled(run, matched) } else { Span::raw(run) });
    }
    let room = width.saturating_sub(h.item.name.chars().count() + 3);
    if room > 8 {
        let parent = ellipsize(&h.item.parent_display(), room.saturating_sub(2));
        spans.push(Span::styled(format!("  {parent}"), DIM));
    }
    ListItem::new(Line::from(spans))
}

/// App-search mode: the one thing ⏎ will do, in place of the result list.
fn draw_app_search(f: &mut Frame, area: Rect, m: &AppSearchMode) {
    let name = m.target.name();
    let block = pane(format!(" {name} "));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let glyph = Span::styled(format!("{SEARCH_GLYPH} "), KEY);
    let terms = m.terms.trim();
    let (head, detail) = if terms.is_empty() {
        let verb = if m.target.app.is_some() { "Open" } else { "Search" };
        (
            Line::from(vec![glyph, Span::raw(format!("{verb} {name}"))]),
            Line::from(Span::styled("  type what to search for", DIM)),
        )
    } else {
        let room = inner.width.saturating_sub(6) as usize;
        (
            Line::from(vec![
                glyph,
                Span::raw(format!("Search {name} for ")),
                Span::styled(format!("\"{terms}\""), KEY),
            ]),
            Line::from(Span::styled(
                format!("  {}", ellipsize(&m.target.url_for(terms), room)),
                DIM,
            )),
        )
    };
    let mut state = ListState::default().with_selected(Some(0));
    f.render_stateful_widget(selectable([ListItem::new(vec![head, detail])]), inner, &mut state);
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
        // Blocks in the accent colour, the drop shadow dimmed.
        let spans: Vec<Span> = runs(row, |_, c| c == '█')
            .into_iter()
            .map(|(run, solid)| Span::styled(run, if solid { KEY } else { DIM }))
            .collect();
        let at = Rect::new(x, inner.y + 1 + i as u16, BANNER_WIDTH, 1);
        f.render_widget(Paragraph::new(Line::from(spans)), at);
    }
    let mut hints = vec![
        keyed(&["type to find apps, documents, folders and media, or ", "2+2"]),
        keyed(&[
            "", "app:", " ", "doc:", " ", "dir:", " scope · ", "in:", "Downloads · ", "ext:", "pdf · ",
            "⇥", " cycles scope",
        ]),
    ];
    if !app.cfg.searches.is_empty() {
        let names: Vec<&str> = app.cfg.searches.iter().map(|s| s.name()).collect();
        let what = format!(" on {} searches inside the app", names.join(" or "));
        hints.push(keyed(&["", "⇥", &what]));
    }
    for (i, line) in hints.into_iter().enumerate() {
        let w = line.width() as u16;
        let hx = inner.x + inner.width.saturating_sub(w) / 2;
        f.render_widget(Paragraph::new(line), Rect::new(hx, inner.y + 8 + i as u16, w, 1));
    }
    let list_y = inner.y + 11;
    if list_y < inner.bottom() && !app.hits.is_empty() {
        let list_area = Rect::new(inner.x, list_y, inner.width, inner.bottom() - list_y);
        draw_list(f, list_area, app);
    }
}

/// Zero results for a real query: say so, and offer the refresh that fixes "I just downloaded it".
fn draw_empty(f: &mut Frame, area: Rect, app: &App) {
    let last = app
        .last_indexed
        .map_or_else(|| "never".into(), |t| relative_time(t, seekr_core::now_secs()));
    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("  no matches for "),
            Span::styled(app.line.text().to_string(), Style::new().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  just added or downloaded it?  press "),
            Span::styled("^x", KEY),
            Span::raw(" to refresh the index"),
        ]),
        Line::from(Span::styled(format!("  index last refreshed {last}"), DIM)),
        Line::from(""),
        Line::from(vec![
            Span::styled("  tips  ", DIM),
            Span::raw("fewer letters · drop the scope prefix · "),
            Span::styled("code:", KEY),
            Span::raw(" for source files"),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).block(results_block(app)), area);
}

fn draw_preview(f: &mut Frame, area: Rect, app: &App) {
    let lines = if let Some(m) = &app.app_search {
        app_search_details(m)
    } else if let Some(c) = app.selected_calc() {
        calc_details(c, area.width.saturating_sub(2) as usize, app.in_window())
    } else if let Some(h) = app.selected_hit() {
        item_details(h, app)
    } else {
        vec![Line::from(Span::styled("no results", DIM))]
    };
    f.render_widget(
        Paragraph::new(lines).block(pane(" details ")).wrap(Wrap { trim: false }),
        area,
    );
}

fn app_search_details(m: &AppSearchMode) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        format!("Search {}", m.target.name()),
        ACCENT,
    ))];
    if let Some(b) = &m.target.app {
        lines.push(field("app", b.clone()));
    }
    let url = if m.terms.trim().is_empty() {
        m.target.url.clone()
    } else {
        m.target.url_for(&m.terms)
    };
    lines.push(field("url", url));
    lines.push(Line::from(""));
    let enter = if m.target.app.is_some() {
        "search · with nothing typed, open the app"
    } else {
        "search"
    };
    lines.push(hints(&[("⏎", enter)]));
    lines.push(hints(&[("^y", "copy url  "), ("⌫", "back when empty  "), ("esc", "clear")]));
    lines
}

fn item_details(h: &Hit, app: &App) -> Vec<Line<'static>> {
    let it = &h.item;
    let mut lines = vec![Line::from(Span::styled(
        it.name.clone(),
        Style::new().fg(kind_color(it.kind)).add_modifier(Modifier::BOLD),
    ))];
    if let Some(alt) = &it.alt {
        lines.push(field("also", alt.clone()));
    }
    lines.push(field("kind", it.kind.label()));
    lines.push(field("path", it.path_display()));
    if it.kind != Kind::App && it.kind != Kind::Folder {
        lines.push(field("size", human_size(it.size)));
    }
    lines.push(field("edited", relative_time(it.modified, seekr_core::now_secs())));
    if let Some(b) = &it.bundle_id {
        lines.push(field("bundle", b.clone()));
    }
    lines.push(Line::from(""));
    if let Some(s) = it.bundle_id.as_deref().and_then(|b| app.cfg.search_for_app(b)) {
        lines.push(hints(&[("⇥", &format!("search in {}", s.name()))]));
    }
    let esc = if app.in_window() { "clear, then hide  " } else { "clear, then quit  " };
    lines.extend([
        hints(&[("⏎", "open    "), ("^r", "reveal in Finder")]),
        hints(&[("^y", "copy path  "), ("^v", "quick look")]),
        hints(&[("^e", "open folder  "), ("^x", "refresh index")]),
        hints(&[("esc", esc), ("^u", "clear  "), ("^w ⌥⌫", "delete word")]),
    ]);
    if std::env::var_os("SEEKR_DEBUG").is_some() {
        lines.push(Line::from(""));
        lines.push(field("score", format!("{:.1}", h.score)));
    }
    lines
}

/// Details for the answer row: the expression, the answer in large digits
/// when they fit, and what ⏎ does.
fn calc_details(c: &Calculation, width: usize, in_window: bool) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled("Calculator", ACCENT)),
        Line::from(Span::styled(c.expr.clone(), DIM)),
        Line::from(""),
    ];
    match big_digits(&c.display).filter(|rows| rows[0].chars().count() + 1 <= width) {
        Some(rows) => {
            for row in rows {
                lines.push(Line::from(Span::styled(format!(" {row}"), ACCENT)));
            }
        }
        None => lines.push(Line::from(Span::styled(format!("= {}", c.display), ACCENT))),
    }
    lines.push(Line::from(""));
    lines.push(field("value", c.raw.clone()));
    lines.push(Line::from(""));
    // In the window ⏎ also hides it; ^y copies and stays.
    lines.push(if in_window {
        hints(&[("⏎", "copy and hide  "), ("^y", "copy  "), ("↓", "results")])
    } else {
        hints(&[("⏎", "copy  "), ("↓", "results")])
    });
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
    let busy = Style::new().fg(Color::Yellow);
    let state = if let Some(err) = app.loader_error() {
        Span::styled(format!(" error: {err} "), Style::new().fg(Color::Red))
    } else if let Some((msg, _)) = &app.status {
        Span::styled(format!(" {msg} "), busy)
    } else if app.refreshing() {
        match app.refresh_added() {
            0 => Span::styled(" refreshing… ", busy),
            added => Span::styled(format!(" refreshing… +{added} new "), busy),
        }
    } else if app.loading() {
        Span::styled(" loading… ", busy)
    } else {
        Span::styled(" ready ", Style::new().fg(Color::Green))
    };
    let esc = if app.in_window() { "esc clear/hide" } else { "esc clear/quit" };
    let keys = if app.app_search.is_some() {
        format!("  ⏎ search · ^y copy url · ⌫ back · {esc}")
    } else if app.selected_calc().is_some() {
        format!("  ⏎ copy answer · ^y copy · ↑↓ results · {esc}")
    } else {
        let dict = if app.definition().is_some() { " · ^l dictionary" } else { "" };
        format!("  ⏎ open · ^r reveal · ^y copy · ^v look · ⇥ search/scope · ^x refresh{dict} · {esc}")
    };
    f.render_widget(Paragraph::new(Line::from(vec![state, Span::styled(keys, DIM)])), area);
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

// ---------------------------------------------------------------------------
// Where the definition card goes.

/// No index hits for a word: the card takes the place of "no matches".
fn definition_replaces_results(app: &App) -> bool {
    app.definition().is_some() && app.hits.is_empty() && app.calc.is_none() && !app.loading()
}

/// No index hits: the card where the results were, as tall as it needs, and
/// the "no matches" note below it when there is room.
fn draw_definition_without_hits(f: &mut Frame, area: Rect, app: &App) {
    let Some(d) = app.definition() else {
        return;
    };
    let need = definition::card_height(d, area.width);
    if area.height < need + 8 {
        definition::draw_card(f, area, d);
        return;
    }
    definition::draw_card(f, Rect::new(area.x, area.y, area.width, need), d);
    draw_empty(f, Rect::new(area.x, area.y + need, area.width, area.height - need), app);
}

/// Split `area` into what was there and, below it, the definition card: as
/// tall as the entry needs, at most `pct`% of `area`, leaving `keep` rows.
fn split_for_definition(area: Rect, app: &App, pct: u16, keep: u16) -> (Rect, Option<Rect>) {
    let Some(d) = app.definition() else {
        return (area, None);
    };
    let cap = (area.height * pct / 100).min(area.height.saturating_sub(keep));
    let h = definition::card_height(d, area.width).min(cap);
    if h < 5 {
        return (area, None);
    }
    let top = Rect::new(area.x, area.y, area.width, area.height - h);
    let card = Rect::new(area.x, area.y + area.height - h, area.width, h);
    (top, Some(card))
}

fn draw_definition(f: &mut Frame, area: Rect, app: &App) {
    if let Some(d) = app.definition() {
        definition::draw_card(f, area, d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_group_neighbours() {
        let r = runs("seekr", |i, _| i == 1 || i == 2 || i == 4);
        assert_eq!(
            r,
            [("s".to_string(), false), ("ee".to_string(), true), ("k".to_string(), false), ("r".to_string(), true)]
        );
        assert!(runs("", |_, _| true).is_empty());
    }

    #[test]
    fn big_digits_have_three_equal_rows() {
        let rows = big_digits("-1,234.5").unwrap();
        let w = rows[0].chars().count();
        assert!(rows.iter().all(|r| r.chars().count() == w), "{rows:?}");
        assert!(big_digits("1.5e20").is_none());
    }
}
