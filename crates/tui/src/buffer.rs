//! A grid of cells.

use crate::style::Style;
use crate::width::char_width;

/// What a cell displays.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Glyph {
    /// One column of text, including any combining marks on that base.
    Single(String),
    /// The first column of a two-column scalar.
    Wide(String),
    /// The second column of a [`Glyph::Wide`].
    Tail,
}

/// One cell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    /// The glyph in this cell.
    pub glyph: Glyph,
    /// Style drawn with that glyph.
    pub style: Style,
}

impl Cell {
    /// A single space and the default style.
    pub fn blank() -> Self {
        Cell {
            glyph: Glyph::Single(" ".to_owned()),
            style: Style::default(),
        }
    }
}

/// Styled text borrowed for one draw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span<'a> {
    /// Style for every scalar in `content`.
    pub style: Style,
    /// Text drawn from the cursor.
    pub content: &'a str,
}

/// A horizontal run of spans.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Line<'a> {
    /// Spans in order.
    pub spans: Vec<Span<'a>>,
}

/// A rectangular grid. The origin is column 0, row 0.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Buffer {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

impl Buffer {
    /// `width` by `height` spaces.
    pub fn empty(width: u16, height: u16) -> Self {
        let count = usize::from(width) * usize::from(height);
        Buffer {
            width,
            height,
            cells: vec![Cell::blank(); count],
        }
    }

    /// Column count.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Row count.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// The cell at `x`, `y`, when that position is inside the grid.
    pub fn get(&self, x: u16, y: u16) -> Option<&Cell> {
        self.index(x, y).map(|index| &self.cells[index])
    }

    /// Write `span` at `x`, `y`, using at most `columns`.
    ///
    /// Scalars that do not fit are dropped. This does not insert an ellipsis;
    /// call [`crate::truncate`] when the caller wants one. Returns the columns
    /// advanced.
    pub fn set_span(&mut self, x: u16, y: u16, span: &Span<'_>, columns: u16) -> u16 {
        self.write_chars(x, y, span.content, span.style, columns)
    }

    /// Write `line` at `x`, `y`, using at most `columns`.
    pub fn set_line(&mut self, x: u16, y: u16, line: &Line<'_>, columns: u16) -> u16 {
        let mut column = x;
        let mut left = columns;
        for span in &line.spans {
            let used = self.set_span(column, y, span, left);
            column = column.saturating_add(used);
            left = left.saturating_sub(used);
        }
        columns.saturating_sub(left)
    }

    pub(crate) fn cells(&self) -> &[Cell] {
        &self.cells
    }

    fn write_chars(&mut self, x: u16, y: u16, text: &str, style: Style, columns: u16) -> u16 {
        if y < self.height && columns > 0 && x < self.width {
            let limit = x.saturating_add(columns).min(self.width);
            let mut column = x;
            for ch in text.chars() {
                let width = u16::from(char_width(ch));
                if width == 0 {
                    self.attach(column, y, ch);
                } else if column.saturating_add(width) > limit {
                    break;
                } else {
                    let glyph = if width == 2 {
                        Glyph::Wide(ch.to_string())
                    } else {
                        Glyph::Single(ch.to_string())
                    };
                    self.place(column, y, glyph, style);
                    column = column.saturating_add(width);
                }
            }
            column.saturating_sub(x)
        } else {
            0
        }
    }

    fn place(&mut self, x: u16, y: u16, glyph: Glyph, style: Style) {
        let wide = matches!(glyph, Glyph::Wide(_));
        self.break_at(x, y);
        if wide {
            self.break_at(x + 1, y);
        }
        self.assign(x, y, glyph, style);
        if wide {
            self.assign(x + 1, y, Glyph::Tail, style);
        }
    }

    fn break_at(&mut self, x: u16, y: u16) {
        match self.kind_at(x, y) {
            Kind::Wide => self.assign(x + 1, y, Glyph::Single(" ".to_owned()), Style::default()),
            Kind::Tail => self.assign(
                x.saturating_sub(1),
                y,
                Glyph::Single(" ".to_owned()),
                Style::default(),
            ),
            Kind::Single => {}
        }
    }

    fn assign(&mut self, x: u16, y: u16, glyph: Glyph, style: Style) {
        let index = self.offset(x, y);
        self.cells[index] = Cell { glyph, style };
    }

    fn attach(&mut self, column: u16, y: u16, ch: char) {
        if let Some(base) = self.base_column(column, y) {
            let index = self.offset(base, y);
            append_mark(&mut self.cells[index].glyph, ch);
        }
    }

    fn base_column(&self, column: u16, y: u16) -> Option<u16> {
        if column == 0 {
            None
        } else {
            let previous = column - 1;
            match self.kind_at(previous, y) {
                Kind::Tail => previous.checked_sub(1),
                Kind::Single | Kind::Wide => Some(previous),
            }
        }
    }

    fn kind_at(&self, x: u16, y: u16) -> Kind {
        match &self.cells[self.offset(x, y)].glyph {
            Glyph::Single(_) => Kind::Single,
            Glyph::Wide(_) => Kind::Wide,
            Glyph::Tail => Kind::Tail,
        }
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        if x < self.width && y < self.height {
            Some(self.offset(x, y))
        } else {
            None
        }
    }

    fn offset(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.width) + usize::from(x)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
    Single,
    Wide,
    Tail,
}

pub(crate) fn append_mark(glyph: &mut Glyph, ch: char) {
    match glyph {
        Glyph::Single(text) | Glyph::Wide(text) => text.push(ch),
        Glyph::Tail => {}
    }
}
