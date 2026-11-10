//! Painting the finder's frame: cell backgrounds, text in the system's
//! monospaced font, box-drawing and block characters as shapes
//! ([`super::glyphs`]), the two Nerd Font icons the finder uses as SF Symbols,
//! the caret, and the rounded panel behind it all. The colours are kitty's
//! defaults, which is what the finder looked like in the kitty launcher.
use super::glyphs;
use super::grid::Grid;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{sel, AnyThread, ClassType};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSCompositingOperation, NSFont, NSFontWeightBold, NSFontWeightRegular,
    NSGraphicsContext, NSImage, NSImageSymbolConfiguration, NSRectFillUsingOperation,
    NSUnderlineStyleAttributeName, NSFontAttributeName,
};
use objc2_core_foundation::{CFAttributedString, CFString, CGAffineTransform};
use objc2_core_graphics::CGContext;
use objc2_core_text::{kCTForegroundColorFromContextAttributeName, CTLine};
use objc2_foundation::{NSAttributedString, NSDictionary, NSNumber, NSPoint, NSRect, NSSize, NSString};
use ratatui::buffer::Cell;
use ratatui::style::{Color, Modifier};
use std::cell::RefCell;
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

pub type Rgb = (u8, u8, u8);

pub const FOREGROUND: Rgb = (0xdd, 0xdd, 0xdd);
pub const BACKGROUND: Rgb = (0x00, 0x00, 0x00);
const CARET: Rgb = (0xcc, 0xcc, 0xcc);
const BORDER: Rgb = (0x3a, 0x3a, 0x3c);
/// kitty's default 16 colours.
const ANSI: [Rgb; 16] = [
    (0x00, 0x00, 0x00), (0xcc, 0x04, 0x03), (0x19, 0xcb, 0x00), (0xce, 0xcb, 0x00),
    (0x0d, 0x73, 0xcc), (0xcb, 0x1e, 0xd1), (0x0d, 0xcd, 0xcd), (0xdd, 0xdd, 0xdd),
    (0x76, 0x76, 0x76), (0xf2, 0x20, 0x1f), (0x23, 0xfd, 0x00), (0xff, 0xfd, 0x00),
    (0x1a, 0x8f, 0xff), (0xfd, 0x28, 0xff), (0x14, 0xff, 0xff), (0xff, 0xff, 0xff),
];
const FONT_SIZE: f64 = 13.0;
const CORNER_RADIUS: f64 = 12.0;
/// Space between the panel's edge and the grid.
pub const PADDING: NSSize = NSSize { width: 12.0, height: 10.0 };

/// The Nerd Font icons the finder draws, and the SF Symbol for each.
fn symbol_name(ch: char) -> Option<&'static str> {
    match ch {
        '\u{f002}' => Some("magnifyingglass"),
        '\u{f02d}' => Some("book"),
        _ => None,
    }
}

pub fn rgb(color: Color, default: Rgb) -> Rgb {
    match color {
        Color::Reset => default,
        Color::Black => ANSI[0],
        Color::Red => ANSI[1],
        Color::Green => ANSI[2],
        Color::Yellow => ANSI[3],
        Color::Blue => ANSI[4],
        Color::Magenta => ANSI[5],
        Color::Cyan => ANSI[6],
        Color::Gray => ANSI[7],
        Color::DarkGray => ANSI[8],
        Color::LightRed => ANSI[9],
        Color::LightGreen => ANSI[10],
        Color::LightYellow => ANSI[11],
        Color::LightBlue => ANSI[12],
        Color::LightMagenta => ANSI[13],
        Color::LightCyan => ANSI[14],
        Color::White => ANSI[15],
        Color::Indexed(i) => indexed(i),
        Color::Rgb(r, g, b) => (r, g, b),
    }
}

/// The xterm 256-colour palette.
fn indexed(i: u8) -> Rgb {
    match i {
        0..=15 => ANSI[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            (level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            (v, v, v)
        }
    }
}

/// How a cell looks once its style is resolved; `bg` is None for the panel's own.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Look {
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub underline: bool,
}

pub fn look(cell: &Cell) -> Look {
    let mut fg = rgb(cell.fg, FOREGROUND);
    let mut bg = (cell.bg != Color::Reset).then(|| rgb(cell.bg, BACKGROUND));
    if cell.modifier.contains(Modifier::REVERSED) {
        (fg, bg) = (bg.unwrap_or(BACKGROUND), Some(fg));
    }
    Look {
        fg,
        bg,
        bold: cell.modifier.contains(Modifier::BOLD),
        underline: cell.modifier.contains(Modifier::UNDERLINED),
    }
}

pub struct Painter {
    regular: Retained<NSFont>,
    bold: Retained<NSFont>,
    /// One cell, in points.
    pub cell: NSSize,
    /// From a cell's top to the text's baseline.
    baseline: f64,
    colors: RefCell<HashMap<Rgb, Retained<NSColor>>>,
    attributes: RefCell<HashMap<(bool, bool), Retained<NSDictionary<NSString, AnyObject>>>>,
    symbols: RefCell<HashMap<(char, Rgb), Option<Retained<NSImage>>>>,
}

impl Painter {
    pub fn new() -> Painter {
        // SAFETY: the weight constants are plain extern statics.
        let (regular, bold) = unsafe {
            (
                NSFont::monospacedSystemFontOfSize_weight(FONT_SIZE, NSFontWeightRegular),
                NSFont::monospacedSystemFontOfSize_weight(FONT_SIZE, NSFontWeightBold),
            )
        };
        let ascent = regular.ascender();
        let natural = ascent - regular.descender() + regular.leading();
        let height = natural.ceil();
        Painter {
            cell: NSSize::new(regular.maximumAdvancement().width, height),
            baseline: ascent + (height - natural) / 2.0,
            regular,
            bold,
            colors: RefCell::default(),
            attributes: RefCell::default(),
            symbols: RefCell::default(),
        }
    }

    fn color(&self, c: Rgb) -> Retained<NSColor> {
        self.colors
            .borrow_mut()
            .entry(c)
            .or_insert_with(|| {
                let f = |v: u8| v as f64 / 255.0;
                NSColor::colorWithSRGBRed_green_blue_alpha(f(c.0), f(c.1), f(c.2), 1.0)
            })
            .clone()
    }

    /// Text attributes for CoreText: the font, an underline, and the colour
    /// taken from the context (set per run), so one dictionary serves all colours.
    fn attributes(&self, bold: bool, underline: bool) -> Retained<NSDictionary<NSString, AnyObject>> {
        self.attributes
            .borrow_mut()
            .entry((bold, underline))
            .or_insert_with(|| {
                let font: &AnyObject = if bold { &self.bold } else { &self.regular };
                let yes = NSNumber::new_bool(true);
                let single = NSNumber::new_isize(1);
                // SAFETY: extern statics of string constants; CFString and
                // NSString are toll-free bridged.
                let from_context: &NSString =
                    unsafe { &*(kCTForegroundColorFromContextAttributeName as *const CFString as *const NSString) };
                let (font_key, underline_key) = unsafe { (NSFontAttributeName, NSUnderlineStyleAttributeName) };
                let mut keys: Vec<&NSString> = vec![font_key, from_context];
                let mut values: Vec<&AnyObject> = vec![font, &yes];
                if underline {
                    keys.push(underline_key);
                    values.push(&single);
                }
                NSDictionary::from_slices(&keys, &values)
            })
            .clone()
    }

    /// The SF Symbol standing in for `ch`, in `fg`; None when macOS has no
    /// colouring for symbols (before macOS 12) or no such symbol.
    fn symbol(&self, ch: char, fg: Rgb) -> Option<Retained<NSImage>> {
        self.symbols
            .borrow_mut()
            .entry((ch, fg))
            .or_insert_with(|| {
                let name = symbol_name(ch)?;
                // A class method, so it is the metaclass that answers.
                let config_class: &AnyClass = NSImageSymbolConfiguration::class().metaclass();
                if !config_class.responds_to(sel!(configurationWithHierarchicalColor:)) {
                    return None;
                }
                let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)?;
                let size = unsafe { NSImageSymbolConfiguration::configurationWithPointSize_weight(FONT_SIZE - 2.0, NSFontWeightRegular) };
                let tint = NSImageSymbolConfiguration::configurationWithHierarchicalColor(&self.color(fg));
                image.imageWithSymbolConfiguration(&size.configurationByApplyingConfiguration(&tint))
            })
            .clone()
    }

    /// Paint `grid` into the current graphics context: a flipped view of `bounds`.
    pub fn paint(&self, grid: &Grid, bounds: NSRect, scale: f64) {
        let panel = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, CORNER_RADIUS, CORNER_RADIUS);
        self.color(BACKGROUND).setFill();
        panel.fill();
        let Some(context) = NSGraphicsContext::currentContext() else {
            return;
        };
        NSGraphicsContext::saveGraphicsState_class();
        panel.addClip();
        let cg = context.CGContext();
        // CoreText draws upright in a flipped view only with its text matrix flipped too.
        CGContext::set_text_matrix(Some(&cg), CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: 0.0, ty: 0.0 });
        let snap = |v: f64| (v * scale).round() / scale;
        let buf = grid.buffer();
        let (cw, ch) = (self.cell.width, self.cell.height);
        let left = |x: u16| PADDING.width + x as f64 * cw;
        let top = |y: u16| PADDING.height + y as f64 * ch;
        let fill = |c: &NSColor, x: f64, y: f64, w: f64, h: f64| {
            c.setFill();
            let (x0, y0) = (snap(x), snap(y));
            let r = NSRect::new(NSPoint::new(x0, y0), NSSize::new(snap(x + w) - x0, snap(y + h) - y0));
            NSRectFillUsingOperation(r, NSCompositingOperation::SourceOver);
        };

        for y in 0..buf.area.height {
            // Backgrounds, in runs of one colour.
            let mut x = 0;
            while x < buf.area.width {
                let bg = look(&buf[(x, y)]).bg;
                let start = x;
                while x < buf.area.width && look(&buf[(x, y)]).bg == bg {
                    x += 1;
                }
                if let Some(bg) = bg {
                    fill(&self.color(bg), left(start), top(y), (x - start) as f64 * cw, ch);
                }
            }
            // Text: runs of ASCII in one look go to CoreText whole; anything
            // else is placed in its own cell, so a glyph from a fallback font
            // with another width cannot push the rest of the row out of the grid.
            let mut run = String::new();
            let mut run_look: Option<Look> = None;
            let mut run_x = 0;
            let mut x = 0;
            while x < buf.area.width {
                let cell = &buf[(x, y)];
                let sym = cell.symbol();
                let l = look(cell);
                let width = UnicodeWidthStr::width(sym).max(1) as u16;
                let ascii = sym.len() == 1 && (sym.as_bytes()[0] == b' ' || sym.as_bytes()[0].is_ascii_graphic());
                if !(ascii && run_look.is_some_and(|r| same_text_look(r, l))) {
                    if let Some(r) = run_look.take() {
                        self.text(&cg, &run, r, left(run_x), top(y));
                        run.clear();
                    }
                }
                if ascii {
                    if run_look.is_none() {
                        run_look = Some(l);
                        run_x = x;
                    }
                    run.push_str(sym);
                } else if let Some(c) = sym.chars().next().filter(|_| sym.chars().count() == 1) {
                    if let Some(shapes) = glyphs::shapes(c, cw, ch, 1.0) {
                        for s in shapes {
                            let color = self.color(l.fg);
                            let color = if s.alpha < 1.0 { color.colorWithAlphaComponent(s.alpha) } else { color };
                            fill(&color, left(x) + s.x, top(y) + s.y, s.w, s.h);
                        }
                    } else if let Some(image) = self.symbol(c, l.fg) {
                        let size = image.size();
                        let at = NSRect::new(
                            NSPoint::new(left(x) + (cw - size.width) / 2.0, top(y) + (ch - size.height) / 2.0),
                            size,
                        );
                        // SAFETY: drawing an image into the current context.
                        unsafe {
                            image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                                at,
                                NSRect::ZERO,
                                NSCompositingOperation::SourceOver,
                                1.0,
                                true,
                                None,
                            );
                        }
                    } else {
                        self.text(&cg, sym, l, left(x), top(y));
                    }
                } else if !sym.is_empty() {
                    self.text(&cg, sym, l, left(x), top(y));
                }
                x += width;
            }
            if let Some(r) = run_look {
                self.text(&cg, &run, r, left(run_x), top(y));
            }
        }

        if let Some(p) = grid.caret() {
            fill(&self.color(CARET), left(p.x), top(p.y) + 1.0, 2.0, ch - 2.0);
        }
        NSGraphicsContext::restoreGraphicsState_class();

        let edge = NSRect::new(
            NSPoint::new(bounds.origin.x + 0.5, bounds.origin.y + 0.5),
            NSSize::new(bounds.size.width - 1.0, bounds.size.height - 1.0),
        );
        let border = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(edge, CORNER_RADIUS - 0.5, CORNER_RADIUS - 0.5);
        border.setLineWidth(1.0);
        self.color(BORDER).setStroke();
        border.stroke();
    }

    /// Draw `text` with its baseline in the cell row whose top is `y`.
    fn text(&self, cg: &CGContext, text: &str, l: Look, x: f64, y: f64) {
        if text.trim().is_empty() && !l.underline {
            return;
        }
        let attrs = self.attributes(l.bold, l.underline);
        // SAFETY: a fresh attributed string with valid attributes; NSAttributedString
        // and CFAttributedString are toll-free bridged.
        unsafe {
            let s = NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(text),
                Some(&attrs),
            );
            let cf = &*(&*s as *const NSAttributedString as *const CFAttributedString);
            let line = CTLine::with_attributed_string(cf);
            self.color(l.fg).set();
            CGContext::set_text_position(Some(cg), x, y + self.baseline);
            line.draw(cg);
        }
    }
}

/// Whether two cells can share a CoreText run: same colour, weight and underline.
fn same_text_look(a: Look, b: Look) -> bool {
    a.fg == b.fg && a.bold == b.bold && a.underline == b.underline
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    #[test]
    fn colours() {
        assert_eq!(rgb(Color::Reset, FOREGROUND), FOREGROUND);
        assert_eq!(rgb(Color::Cyan, FOREGROUND), (0x0d, 0xcd, 0xcd));
        assert_eq!(rgb(Color::DarkGray, FOREGROUND), (0x76, 0x76, 0x76));
        assert_eq!(rgb(Color::Rgb(40, 44, 52), FOREGROUND), (40, 44, 52));
        assert_eq!(indexed(16), (0, 0, 0));
        assert_eq!(indexed(231), (255, 255, 255));
        assert_eq!(indexed(196), (255, 0, 0));
        assert_eq!(indexed(232), (8, 8, 8));
        assert_eq!(indexed(255), (238, 238, 238));
    }

    #[test]
    fn looks() {
        let mut cell = Cell::default();
        assert_eq!(look(&cell), Look { fg: FOREGROUND, bg: None, bold: false, underline: false });
        cell.set_style(Style::new().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD));
        let l = look(&cell);
        assert_eq!((l.fg, l.bg, l.bold), (ANSI[0], Some(ANSI[6]), true));
        let mut cell = Cell::default();
        cell.set_style(Style::new().fg(Color::Red).add_modifier(Modifier::REVERSED));
        assert_eq!((look(&cell).fg, look(&cell).bg), (BACKGROUND, Some(ANSI[1])));
    }
}
