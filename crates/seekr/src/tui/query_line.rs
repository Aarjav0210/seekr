//! The query as a text field: text, caret, word moves and deletes, and the
//! whole-text selection a hotkey hide leaves behind.
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

/// What a key did to the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Not a key for the line.
    Ignored,
    /// The caret moved or the selection ended; the text is the same.
    Moved,
    /// The text changed.
    Changed,
}

/// Modifiers that make a character key a chord instead of text. ⌘ is one of
/// them: a terminal speaking the kitty keyboard protocol (kitty, the
/// launcher's host) reports ⌘T as a `t` with SUPER, and it must not type a "t".
const CHORD: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SUPER)
    .union(KeyModifiers::HYPER)
    .union(KeyModifiers::META);

#[derive(Debug, Default)]
pub struct QueryLine {
    text: String,
    /// Caret position, in chars.
    cursor: usize,
    /// Chars at the start the caret can't enter: the `arc: ` keyword that the
    /// app-search chip stands for.
    locked: usize,
    /// The whole text is selected: the next character replaces it.
    selected: bool,
}

impl QueryLine {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The text after the caret.
    pub fn after_cursor(&self) -> &str {
        &self.text[self.byte_at(self.cursor)..]
    }

    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// Replace the text; the caret goes to the end.
    pub fn set(&mut self, text: String) {
        self.cursor = text.chars().count();
        self.text = text;
        self.selected = false;
    }

    /// Keep the caret out of the first `chars` chars.
    pub fn lock(&mut self, chars: usize) {
        self.locked = chars.min(self.len());
        self.cursor = self.cursor.max(self.locked);
    }

    /// Select the whole text (if there is any), like a text field that gets
    /// the focus back.
    pub fn select_all(&mut self) {
        self.selected = !self.text.is_empty();
    }

    /// A key while the whole text is selected: a character replaces it, ⌫
    /// clears it, ← puts the caret at the start. Every key ends the
    /// selection; `None` for the keys that otherwise act as usual.
    pub fn key_on_selection(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<Edit> {
        if !std::mem::take(&mut self.selected) {
            return None;
        }
        match code {
            KeyCode::Char(ch) if !mods.intersects(CHORD) => {
                self.set(ch.to_string());
                Some(Edit::Changed)
            }
            KeyCode::Backspace if !mods.contains(KeyModifiers::ALT) => {
                self.set(String::new());
                Some(Edit::Changed)
            }
            KeyCode::Left => Some(self.move_to(0)),
            _ => None,
        }
    }

    /// Caret movement and editing keys.
    pub fn key(&mut self, code: KeyCode, mods: KeyModifiers) -> Edit {
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let alt = mods.contains(KeyModifiers::ALT);
        // ⌘ arrows arrive as SUPER (contrib/kitty/seekr.conf maps them).
        let cmd = mods.contains(KeyModifiers::SUPER);
        let (start, end, at) = (self.locked, self.len(), self.cursor);
        match code {
            KeyCode::Left if cmd => self.move_to(start),
            KeyCode::Right if cmd => self.move_to(end),
            KeyCode::Left if alt || ctrl => self.move_to(self.word_start()),
            KeyCode::Right if alt || ctrl => self.move_to(self.word_end()),
            KeyCode::Char('b') if alt => self.move_to(self.word_start()),
            KeyCode::Char('f') if alt => self.move_to(self.word_end()),
            KeyCode::Left => self.move_to(at.saturating_sub(1)),
            KeyCode::Right => self.move_to(at + 1),
            KeyCode::Char('a') if ctrl => self.move_to(start),
            KeyCode::Char('u') if ctrl => self.splice(start, at, ""),
            KeyCode::Char('w') if ctrl => self.splice(self.word_start(), at, ""),
            KeyCode::Backspace if alt => self.splice(self.word_start(), at, ""),
            KeyCode::Backspace if cmd => self.splice(start, at, ""),
            KeyCode::Backspace if at > start => self.splice(at - 1, at, ""),
            KeyCode::Delete if at < end => self.splice(at, at + 1, ""),
            KeyCode::Char(ch) if !mods.intersects(CHORD) => self.splice(at, at, ch.encode_utf8(&mut [0; 4])),
            _ => Edit::Ignored,
        }
    }

    /// Put the caret at char `to`, kept between the locked prefix and the end.
    pub fn move_to(&mut self, to: usize) -> Edit {
        self.cursor = to.min(self.len()).max(self.locked);
        Edit::Moved
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of char position `c`.
    fn byte_at(&self, c: usize) -> usize {
        self.text.char_indices().nth(c).map_or(self.text.len(), |(i, _)| i)
    }

    /// Replace chars `from..to` with `text` and leave the caret after it.
    fn splice(&mut self, from: usize, to: usize, text: &str) -> Edit {
        let (a, b) = (self.byte_at(from), self.byte_at(to));
        if a == b && text.is_empty() {
            return self.move_to(from);
        }
        self.text.replace_range(a..b, text);
        self.selected = false;
        self.move_to(from + text.chars().count());
        Edit::Changed
    }

    /// Where the word before the caret starts (`⌥←`, `^w`): skip spaces, then
    /// non-spaces.
    fn word_start(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i.max(self.locked)
    }

    /// Where the word after the caret ends (`⌥→`).
    fn word_end(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const CMD: KeyModifiers = KeyModifiers::SUPER;

    fn line(text: &str) -> QueryLine {
        let mut l = QueryLine::default();
        l.set(text.to_string());
        l
    }

    fn typed(l: &mut QueryLine, s: &str) {
        for ch in s.chars() {
            assert_eq!(l.key(KeyCode::Char(ch), NONE), Edit::Changed);
        }
    }

    /// The text with the caret drawn as `|`.
    fn shown(l: &QueryLine) -> String {
        let (a, b) = l.text.split_at(l.byte_at(l.cursor));
        format!("{a}|{b}")
    }

    #[test]
    fn typing_and_moving_the_caret() {
        let mut l = line("");
        typed(&mut l, "ofer");
        assert_eq!(shown(&l), "ofer|");
        l.key(KeyCode::Left, NONE);
        l.key(KeyCode::Left, NONE);
        typed(&mut l, "f");
        assert_eq!(shown(&l), "off|er");
        assert_eq!(l.after_cursor(), "er");
        l.key(KeyCode::Right, NONE);
        l.key(KeyCode::Right, NONE);
        assert_eq!(l.key(KeyCode::Right, NONE), Edit::Moved);
        assert_eq!(shown(&l), "offer|");
        l.key(KeyCode::Char('a'), CTRL);
        assert_eq!(shown(&l), "|offer");
        assert_eq!(l.key(KeyCode::Backspace, NONE), Edit::Ignored, "nothing before the caret");
        assert_eq!(l.key(KeyCode::Delete, NONE), Edit::Changed);
        assert_eq!(shown(&l), "|ffer");
        l.key(KeyCode::Right, CMD);
        assert_eq!(shown(&l), "ffer|");
        assert_eq!(l.key(KeyCode::Delete, NONE), Edit::Ignored, "nothing after the caret");
        l.key(KeyCode::Left, CMD);
        assert_eq!(shown(&l), "|ffer");
    }

    #[test]
    fn words() {
        let mut l = line("offer letter  in:Downloads");
        l.key(KeyCode::Left, ALT);
        assert_eq!(shown(&l), "offer letter  |in:Downloads");
        l.key(KeyCode::Char('b'), ALT);
        assert_eq!(shown(&l), "offer |letter  in:Downloads");
        l.key(KeyCode::Left, CTRL);
        assert_eq!(shown(&l), "|offer letter  in:Downloads");
        l.key(KeyCode::Right, ALT);
        assert_eq!(shown(&l), "offer| letter  in:Downloads");
        l.key(KeyCode::Char('f'), ALT);
        assert_eq!(shown(&l), "offer letter|  in:Downloads");
        l.key(KeyCode::Right, CTRL);
        assert_eq!(shown(&l), "offer letter  in:Downloads|");

        assert_eq!(l.key(KeyCode::Char('w'), CTRL), Edit::Changed);
        assert_eq!(shown(&l), "offer letter  |");
        l.key(KeyCode::Backspace, ALT);
        assert_eq!(shown(&l), "offer |");
        l.set("a b c".into());
        l.key(KeyCode::Left, NONE);
        l.key(KeyCode::Char('u'), CTRL);
        assert_eq!(shown(&l), "|c");
        assert_eq!(l.key(KeyCode::Char('u'), CTRL), Edit::Moved, "nothing to delete");
        l.set("a b c".into());
        l.key(KeyCode::Backspace, CMD);
        assert_eq!(shown(&l), "|");
    }

    #[test]
    fn a_command_chord_types_nothing() {
        // ⌘T as a kitty-protocol terminal reports it: `t` with SUPER.
        let mut l = line("offer");
        assert_eq!(l.key(KeyCode::Char('t'), CMD), Edit::Ignored);
        assert_eq!(l.key(KeyCode::Char('A'), CMD | KeyModifiers::SHIFT), Edit::Ignored);
        assert_eq!(l.key(KeyCode::Char('1'), CMD), Edit::Ignored);
        assert_eq!(shown(&l), "offer|");
        // ⇧ alone is still just a capital.
        assert_eq!(l.key(KeyCode::Char('S'), KeyModifiers::SHIFT), Edit::Changed);
        assert_eq!(shown(&l), "offerS|");

        // Nor does it replace a selected query.
        l.set("offer".into());
        l.select_all();
        assert_eq!(l.key_on_selection(KeyCode::Char('a'), CMD), None);
        assert_eq!(l.key(KeyCode::Char('a'), CMD), Edit::Ignored);
        assert_eq!(l.text(), "offer");
    }

    #[test]
    fn multibyte_text() {
        let mut l = line("naïve 日本");
        l.key(KeyCode::Left, NONE);
        l.key(KeyCode::Backspace, NONE);
        assert_eq!(shown(&l), "naïve |本");
        l.key(KeyCode::Char('b'), ALT);
        l.key(KeyCode::Right, NONE);
        l.key(KeyCode::Right, NONE);
        l.key(KeyCode::Delete, NONE);
        assert_eq!(shown(&l), "na|ve 本");
    }

    #[test]
    fn the_caret_stays_out_of_the_locked_prefix() {
        let mut l = line("arc: rust lifetimes");
        l.lock(5);
        l.key(KeyCode::Left, CMD);
        assert_eq!(shown(&l), "arc: |rust lifetimes");
        l.key(KeyCode::Char('a'), CTRL);
        l.key(KeyCode::Left, NONE);
        l.key(KeyCode::Left, ALT);
        assert_eq!(shown(&l), "arc: |rust lifetimes");
        assert_eq!(l.key(KeyCode::Backspace, NONE), Edit::Ignored);
        l.key(KeyCode::Right, ALT);
        l.key(KeyCode::Char('w'), CTRL);
        assert_eq!(shown(&l), "arc: | lifetimes");
        l.key(KeyCode::Right, CMD);
        l.key(KeyCode::Char('u'), CTRL);
        assert_eq!(shown(&l), "arc: |");
        l.lock(99);
        assert_eq!(l.locked, 5, "never past the end");
    }

    #[test]
    fn a_selected_line_is_replaced_by_typing() {
        let mut l = line("offer");
        l.select_all();
        assert_eq!(l.key_on_selection(KeyCode::Char('x'), NONE), Some(Edit::Changed));
        assert_eq!(shown(&l), "x|");
        assert!(!l.is_selected());
        assert_eq!(l.key_on_selection(KeyCode::Char('y'), NONE), None, "selection is gone");

        l.set("offer".into());
        l.select_all();
        assert_eq!(l.key_on_selection(KeyCode::Backspace, NONE), Some(Edit::Changed));
        assert_eq!(shown(&l), "|");

        l.set("offer".into());
        l.select_all();
        assert_eq!(l.key_on_selection(KeyCode::Left, NONE), Some(Edit::Moved));
        assert_eq!(shown(&l), "|offer");

        // Other keys keep the text and act as usual: ⌥b moves by word.
        l.set("offer letter".into());
        l.select_all();
        assert_eq!(l.key_on_selection(KeyCode::Char('b'), ALT), None);
        assert!(!l.is_selected());
        l.key(KeyCode::Char('b'), ALT);
        assert_eq!(shown(&l), "offer |letter");
        l.set("offer".into());
        l.select_all();
        assert_eq!(l.key_on_selection(KeyCode::Enter, NONE), None);
        assert_eq!(l.text(), "offer");

        let mut empty = line("");
        empty.select_all();
        assert!(!empty.is_selected(), "nothing to select");
    }
}
