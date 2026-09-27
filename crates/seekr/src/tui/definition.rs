//! The dictionary card: the definition of a one-word query, like Spotlight's.
//! [`DictLookup`] looks the word up off the UI thread; [`draw_card`] renders
//! the entry as detailed as the space allows.
use super::view::{pane, DIM, KEY};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Padding, Paragraph};
use ratatui::Frame;
use seekr_core::define::{self, Definition, Sense};
use std::sync::mpsc;
use std::time::Duration;

/// Wait this long for the next keystroke before looking a word up.
const DEFINE_DEBOUNCE: Duration = Duration::from_millis(100);
/// Nerd Font book, for the card's title.
const DICT_GLYPH: &str = "\u{f02d}";

/// Dictionary lookups for the query, on a worker thread so typing never waits
/// (a lookup takes a few ms, the first one in a process 100–250 ms).
pub struct DictLookup {
    tx: mpsc::Sender<String>,
    rx: mpsc::Receiver<(String, Option<Definition>)>,
    /// The word the query asks to define.
    word: Option<String>,
    /// Its entry, once the lookup has landed.
    entry: Option<Definition>,
}

impl DictLookup {
    pub fn spawn() -> DictLookup {
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
    pub fn want(&mut self, word: Option<&str>) {
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
    pub fn poll(&mut self) {
        while let Ok((word, entry)) = self.rx.try_recv() {
            if self.word.as_deref() == Some(word.as_str()) {
                self.entry = entry;
            }
        }
    }

    /// The entry for the word asked for last, once it has landed.
    pub fn entry(&self) -> Option<&Definition> {
        self.entry.as_ref()
    }
}

/// Rows a card `width` columns wide needs to show all of `d`.
pub fn card_height(d: &Definition, width: u16) -> u16 {
    let width = width.saturating_sub(4) as usize;
    let lines = definition_lines(d, width, 3).len() + origin_lines(d, width).len() + 1;
    lines.min(u16::MAX as usize - 2) as u16 + 2
}

pub fn draw_card(f: &mut Frame, area: Rect, d: &Definition) {
    let block = pane(format!(" {DICT_GLYPH} definition "))
        .title_bottom(
            Line::from(vec![
                Span::styled(" ^l", KEY),
                Span::styled(" open in Dictionary ", DIM),
            ])
            .right_aligned(),
        )
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
    let mut out = Vec::new();
    let mut head = vec![(d.headword.clone(), Style::new().add_modifier(Modifier::BOLD))];
    if let Some(p) = &d.pronunciation {
        head.push((format!("  /{p}/"), DIM));
    }
    if let Some(n) = &d.note {
        head.push((format!("  {n}"), DIM));
    }
    out.extend(wrap_styled(Vec::new(), &head, width, 2));
    for part in &d.parts {
        if level >= 2 {
            out.push(Line::from(""));
        }
        let mut pos = Vec::new();
        if let Some(name) = &part.name {
            pos.push((name.clone(), Style::new().fg(Color::Magenta).add_modifier(Modifier::ITALIC)));
        }
        if let Some(n) = &part.note {
            let sep = if pos.is_empty() { "" } else { "  " };
            pos.push((format!("{sep}{n}"), DIM));
        }
        if !pos.is_empty() {
            out.extend(wrap_styled(Vec::new(), &pos, width, 2));
        }
        let numbered = part.senses.len() > 1;
        for (i, s) in part.senses.iter().enumerate() {
            let (prefix, indent) = if numbered {
                let n = format!("{} ", i + 1);
                let w = n.len();
                (vec![Span::styled(n, KEY)], w)
            } else {
                (Vec::new(), 0)
            };
            out.extend(wrap_styled(prefix, &sense_segments(s, level >= 1), width, indent));
            if level >= 2 {
                for sub in &s.subsenses {
                    let prefix = vec![Span::raw(" ".repeat(indent)), Span::styled("• ", DIM)];
                    out.extend(wrap_styled(prefix, &sense_segments(sub, level >= 3), width, indent + 2));
                }
            }
        }
    }
    out
}

fn origin_lines(d: &Definition, width: usize) -> Vec<Line<'static>> {
    match &d.origin {
        Some(o) => wrap_styled(
            vec![Span::styled("origin ", DIM.add_modifier(Modifier::BOLD))],
            &[(o.clone(), DIM)],
            width,
            0,
        ),
        None => Vec::new(),
    }
}

/// A sense's text, then (optionally) its first example, dimmed.
fn sense_segments(s: &Sense, example: bool) -> Vec<(String, Style)> {
    let mut segs = vec![(s.text.clone(), Style::new())];
    if example {
        if let Some(e) = s.examples.first() {
            // `[with object] : she ran the bath`: the grammar label is noise here.
            let e = match e.find("] : ") {
                Some(i) if e.starts_with('[') => &e[i + 4..],
                _ => e.as_str(),
            };
            segs.push((": ".into(), Style::new()));
            segs.push((e.to_string(), DIM.add_modifier(Modifier::ITALIC)));
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
    spans.push(Span::styled(" …", DIM));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    /// Columns used, not counting the trailing space a wrapped line may keep.
    fn visible_width(l: &Line) -> usize {
        Span::raw(l.to_string().trim_end().to_string()).width()
    }

    #[test]
    fn wrapping_fits_the_width_and_indents() {
        let segs = [("the occurrence and development of events by chance".to_string(), Style::new())];
        let lines = wrap_styled(vec![Span::raw("1 ")], &segs, 20, 2);
        assert_eq!(
            text(&lines),
            ["1 the occurrence and ", "  development of ", "  events by chance"]
        );
        for l in &lines {
            assert!(visible_width(l) <= 20, "{l:?}");
        }
        // A word longer than the line is split.
        let long = [("antidisestablishmentarianism".to_string(), Style::new())];
        assert_eq!(text(&wrap_styled(Vec::new(), &long, 10, 0)), ["antidisest", "ablishment", "arianism"]);
    }

    #[test]
    fn the_card_never_overflows() {
        let raw = "arc | ɑːk | noun 1 a part of a curve: the arc of a circle. • a shape: the arc of the sky. 2 a luminous electrical discharge: striking the arc. verb (arcs, arcing, arced) [no object] move with a curving trajectory: the ball arced across the room. ORIGIN late Middle English.";
        let d = define::parse("arc", raw).unwrap();
        for (w, h) in [(60, 30), (40, 12), (30, 5), (12, 2), (8, 1)] {
            let lines = fit_definition(&d, w, h);
            assert!(lines.len() <= h, "{w}x{h}: {} lines", lines.len());
            assert!(lines.iter().all(|l| visible_width(l) <= w), "{w}x{h}: {:?}", text(&lines));
        }
        assert!(text(&fit_definition(&d, 60, 30)).iter().any(|l| l.starts_with("origin")));
        let cut = text(&fit_definition(&d, 30, 3));
        assert!(cut.last().unwrap().ends_with(" …"), "{cut:?}");
        assert!(fit_definition(&d, 7, 10).is_empty());
    }
}
