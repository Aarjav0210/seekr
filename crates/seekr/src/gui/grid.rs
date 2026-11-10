//! The terminal the finder draws into inside seekr.app: ratatui's frames land
//! in a buffer of cells that the window paints, instead of going out as
//! escape codes to a pty.
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use std::convert::Infallible;

pub struct Grid {
    buffer: Buffer,
    cursor: Position,
    cursor_shown: bool,
    /// Something changed since [`Grid::take_changed`] last looked.
    changed: bool,
}

impl Grid {
    pub fn new(cols: u16, rows: u16) -> Grid {
        Grid {
            buffer: Buffer::empty(Rect::new(0, 0, cols, rows)),
            cursor: Position::ORIGIN,
            cursor_shown: false,
            changed: true,
        }
    }

    /// A new size, picked up by the finder's next frame.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let area = Rect::new(0, 0, cols, rows);
        if area != self.buffer.area {
            self.buffer.resize(area);
            self.changed = true;
        }
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// The caret, while the frame shows one.
    pub fn caret(&self) -> Option<Position> {
        self.cursor_shown.then_some(self.cursor)
    }

    /// Whether anything changed since the last call: the window only paints then.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
}

impl Backend for Grid {
    type Error = Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Infallible>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            if let Some(c) = self.buffer.cell_mut((x, y)) {
                *c = cell.clone();
                self.changed = true;
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Infallible> {
        self.changed |= self.cursor_shown;
        self.cursor_shown = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Infallible> {
        self.changed |= !self.cursor_shown;
        self.cursor_shown = true;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Infallible> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Infallible> {
        let p = position.into();
        self.changed |= p != self.cursor;
        self.cursor = p;
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Infallible> {
        self.buffer.reset();
        self.changed = true;
        Ok(())
    }

    fn clear_region(&mut self, _: ClearType) -> Result<(), Infallible> {
        self.clear()
    }

    fn size(&self) -> Result<Size, Infallible> {
        Ok(self.buffer.area.as_size())
    }

    fn window_size(&mut self) -> Result<WindowSize, Infallible> {
        Ok(WindowSize { columns_rows: self.buffer.area.as_size(), pixels: Size::default() })
    }

    fn flush(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::Paragraph;
    use ratatui::Terminal;

    #[test]
    fn frames_land_in_the_buffer_with_the_caret() {
        let mut term = Terminal::new(Grid::new(12, 2)).unwrap();
        term.backend_mut().take_changed();
        term.draw(|f| {
            f.render_widget(Paragraph::new("seekr"), f.area());
            f.set_cursor_position((5, 0));
        })
        .unwrap();
        let grid = term.backend_mut();
        assert!(grid.take_changed());
        assert_eq!(grid.buffer()[(0, 0)].symbol(), "s");
        assert_eq!(grid.buffer()[(4, 0)].symbol(), "r");
        assert_eq!(grid.caret(), Some(Position::new(5, 0)));
        assert!(!grid.take_changed(), "nothing new since the last look");

        // The same frame again changes nothing; a frame without a caret hides it.
        term.draw(|f| f.render_widget(Paragraph::new("seekr"), f.area())).unwrap();
        let grid = term.backend_mut();
        assert!(grid.take_changed(), "the caret went away");
        assert_eq!(grid.caret(), None);
    }

    #[test]
    fn a_resize_reaches_the_next_frame() {
        let mut term = Terminal::new(Grid::new(4, 1)).unwrap();
        term.backend_mut().resize(20, 3);
        let mut area = Rect::default();
        term.draw(|f| area = f.area()).unwrap();
        assert_eq!(area, Rect::new(0, 0, 20, 3));
    }
}
