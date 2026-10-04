//! A bordered block, a list, a table, a paragraph, tabs, and a one-line input.
//!
//! The application keeps the selection, the scroll position, and the text
//! being edited. These functions draw that state and update it when the
//! caller asks.

use crate::buffer::{Buffer, Span};
use crate::layout::Rect;
use crate::style::Style;
use crate::width::{char_width, display_width, truncate};

/// Selection and scroll for a [`draw_list`] or [`draw_table`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListState {
    /// Selected item, when one is selected.
    pub selected: Option<usize>,
    /// First visible item.
    pub offset: usize,
}

impl Default for ListState {
    fn default() -> Self {
        Self::new()
    }
}

impl ListState {
    /// Nothing selected, scrolled to the top.
    pub fn new() -> Self {
        ListState {
            selected: None,
            offset: 0,
        }
    }

    /// Move `offset` so `selected` lies in a window of `rows` items.
    pub fn reveal(&mut self, len: usize, rows: usize) {
        self.offset = ensure_visible(self.selected, len, rows, self.offset);
    }
}

/// One-line editor state. `cursor` is a character index.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Input {
    /// The line contents.
    pub value: String,
    /// Characters before the cursor.
    pub cursor: usize,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    /// An empty line and the cursor at the start.
    pub fn new() -> Self {
        Input {
            value: String::new(),
            cursor: 0,
        }
    }

    /// Insert `ch` at the cursor and move the cursor past it.
    pub fn insert(&mut self, ch: char) {
        let cursor = self.cursor.min(self.value.chars().count());
        let at = byte_at(&self.value, cursor);
        self.value.insert(at, ch);
        self.cursor = cursor + 1;
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        let cursor = self.cursor.min(self.value.chars().count());
        if cursor > 0 {
            let end = byte_at(&self.value, cursor);
            let start = byte_at(&self.value, cursor - 1);
            self.value.replace_range(start..end, "");
            self.cursor = cursor - 1;
        } else {
            self.cursor = 0;
        }
    }

    /// Place the cursor at the character under `column` on this line.
    pub fn click(&mut self, area: Rect, column: u16, row: u16) {
        let inside = area.height > 0
            && row == area.y
            && column >= area.x
            && column < area.x.saturating_add(area.width);
        if inside {
            let width = usize::from(area.width);
            let start = visible_start(&self.value, self.cursor, width);
            let shown: String = self.value.chars().skip(start).collect();
            let local = usize::from(column - area.x);
            self.cursor = start + char_index_at(&shown, local);
        }
    }
}

/// `1/40` for the first selected item, or `0/40` when nothing is selected.
pub fn position(selected: Option<usize>, len: usize) -> String {
    match selected {
        Some(index) if index < len => format!("{}/{}", index + 1, len),
        _ => format!("0/{len}"),
    }
}

/// Scroll offset that keeps `selected` inside a window of `rows` items.
pub fn ensure_visible(selected: Option<usize>, len: usize, rows: usize, offset: usize) -> usize {
    let max_offset = len.saturating_sub(rows);
    let clamped = offset.min(max_offset);
    if let Some(index) = selected.filter(|index| *index < len && rows > 0) {
        if index < clamped {
            index
        } else if index >= clamped + rows {
            index + 1 - rows
        } else {
            clamped
        }
    } else {
        clamped
    }
}

/// The item under `column`/`row`, using the list and table body above the position row.
pub fn hit_row(area: Rect, offset: usize, len: usize, column: u16, row: u16) -> Option<usize> {
    let rows = u32::from(area.height.saturating_sub(1));
    let left = u32::from(area.x);
    let top = u32::from(area.y);
    let col = u32::from(column);
    let line = u32::from(row);
    let inside =
        col >= left && col < left + u32::from(area.width) && line >= top && line < top + rows;
    if inside {
        let index = offset + (line - top) as usize;
        if index < len { Some(index) } else { None }
    } else {
        None
    }
}

/// The area inside a one-cell border. A border that does not fit yields an empty rect.
pub fn inner(area: Rect) -> Rect {
    if area.width >= 2 && area.height >= 2 {
        Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: area.width - 2,
            height: area.height - 2,
        }
    } else {
        Rect {
            x: area.x,
            y: area.y,
            width: 0,
            height: 0,
        }
    }
}

/// Draw a border and `title` on the top edge.
pub fn draw_block(
    buffer: &mut Buffer,
    area: Rect,
    title: &str,
    focused: bool,
    border: Style,
    focused_border: Style,
) {
    if area.width >= 2 && area.height >= 2 {
        let style = if focused { focused_border } else { border };
        let right = area.x.saturating_add(area.width - 1);
        let bottom = area.y.saturating_add(area.height - 1);
        let left = area.x.saturating_add(1);
        put(buffer, area.x, area.y, "+", style);
        put(buffer, right, area.y, "+", style);
        put(buffer, area.x, bottom, "+", style);
        put(buffer, right, bottom, "+", style);
        let span = area.width - 2;
        fill(buffer, left, area.y, span, style);
        fill(buffer, left, bottom, span, style);
        let mut y = area.y.saturating_add(1);
        while y < bottom {
            put(buffer, area.x, y, "|", style);
            put(buffer, right, y, "|", style);
            y = y.saturating_add(1);
        }
        let title = truncate(title, usize::from(span));
        buffer.set_span(
            left,
            area.y,
            &Span {
                style,
                content: &title,
            },
            span,
        );
    }
}

/// Draw `items`. The last row is [`position`].
pub fn draw_list(
    buffer: &mut Buffer,
    area: Rect,
    items: &[&str],
    state: &ListState,
    style: Style,
    selected: Style,
) {
    paint_rows(
        buffer,
        area,
        items.len(),
        state,
        style,
        selected,
        |buffer, x, y, index, look, room| {
            let text = fit(items[index], room);
            buffer.set_span(
                x,
                y,
                &Span {
                    style: look,
                    content: &text,
                },
                room,
            );
        },
    );
}

/// Draw `rows` in `widths` columns. The last row is [`position`].
pub fn draw_table(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[&[&str]],
    widths: &[u16],
    state: &ListState,
    style: Style,
    selected: Style,
) {
    paint_rows(
        buffer,
        area,
        rows.len(),
        state,
        style,
        selected,
        |buffer, x, y, index, look, room| {
            draw_columns(buffer, x, y, rows[index], widths, room, look);
        },
    );
}

/// Draw `labels` on the first row. `selected` uses `selected_style`.
pub fn draw_tabs(
    buffer: &mut Buffer,
    area: Rect,
    labels: &[&str],
    selected: usize,
    style: Style,
    selected_style: Style,
) {
    if area.height > 0 && area.width > 0 {
        let end = area.x.saturating_add(area.width);
        let mut column = area.x;
        for (index, label) in labels.iter().enumerate() {
            if column >= end {
                break;
            }
            let look = if index == selected {
                selected_style
            } else {
                style
            };
            let room = end - column;
            let used = buffer.set_span(
                column,
                area.y,
                &Span {
                    style: look,
                    content: label,
                },
                room,
            );
            column = column.saturating_add(used);
            if index + 1 < labels.len() && column < end {
                let gap = buffer.set_span(
                    column,
                    area.y,
                    &Span {
                        style,
                        content: "|",
                    },
                    end - column,
                );
                column = column.saturating_add(gap);
            }
        }
    }
}

/// Draw `text` wrapped to `area`.
pub fn draw_paragraph(buffer: &mut Buffer, area: Rect, text: &str, style: Style) {
    if area.width > 0 && area.height > 0 {
        let lines = wrap(text, usize::from(area.width));
        for (index, line) in lines.iter().enumerate() {
            if index >= usize::from(area.height) {
                break;
            }
            let y = area.y.saturating_add(index as u16);
            buffer.set_span(
                area.x,
                y,
                &Span {
                    style,
                    content: line,
                },
                area.width,
            );
        }
    }
}

/// Draw `input` on the first row and reverse the cursor cell.
pub fn draw_input(buffer: &mut Buffer, area: Rect, input: &Input, style: Style) {
    if area.height > 0 && area.width > 0 {
        let cursor = input.cursor.min(input.value.chars().count());
        let width = usize::from(area.width);
        let start = visible_start(&input.value, cursor, width);
        let shown: String = input.value.chars().skip(start).collect();
        buffer.set_span(
            area.x,
            area.y,
            &Span {
                style,
                content: &shown,
            },
            area.width,
        );
        let column =
            prefix_width(&input.value, cursor).saturating_sub(prefix_width(&input.value, start));
        let column = column.min(usize::from(area.width.saturating_sub(1))) as u16;
        let mut cursor_style = style;
        cursor_style.reverse = true;
        buffer.set_style(area.x.saturating_add(column), area.y, cursor_style);
    }
}

fn paint_rows(
    buffer: &mut Buffer,
    area: Rect,
    len: usize,
    state: &ListState,
    style: Style,
    selected: Style,
    mut paint: impl FnMut(&mut Buffer, u16, u16, usize, Style, u16),
) {
    let rows = usize::from(area.height.saturating_sub(1));
    let offset = state.offset.min(len);
    for row in 0..rows {
        let index = offset + row;
        if index >= len {
            break;
        }
        let look = if state.selected == Some(index) {
            selected
        } else {
            style
        };
        let y = area.y.saturating_add(row as u16);
        paint(buffer, area.x, y, index, look, area.width);
    }
    if area.height > 0 {
        let y = area.y.saturating_add(area.height - 1);
        let label = position(state.selected, len);
        buffer.set_span(
            area.x,
            y,
            &Span {
                style,
                content: &label,
            },
            area.width,
        );
    }
}

fn draw_columns(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    cells: &[&str],
    widths: &[u16],
    room: u16,
    style: Style,
) {
    let end = x.saturating_add(room);
    let mut column = x;
    for (index, cell) in cells.iter().enumerate() {
        if column >= end {
            break;
        }
        let slot = widths.get(index).copied().unwrap_or(0).min(end - column);
        if slot == 0 {
            break;
        }
        let text = fit(cell, slot);
        buffer.set_span(
            column,
            y,
            &Span {
                style,
                content: &text,
            },
            slot,
        );
        column = column.saturating_add(slot).saturating_add(1);
    }
}

fn fit(text: &str, columns: u16) -> String {
    let columns = usize::from(columns);
    if display_width(text) > columns {
        truncate(text, columns)
    } else {
        text.to_owned()
    }
}

fn put(buffer: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    buffer.set_span(
        x,
        y,
        &Span {
            style,
            content: text,
        },
        1,
    );
}

fn fill(buffer: &mut Buffer, x: u16, y: u16, width: u16, style: Style) {
    let mut column = x;
    for _ in 0..width {
        put(buffer, column, y, "-", style);
        column = column.saturating_add(1);
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
        } else {
            let mut rest = paragraph;
            while !rest.is_empty() {
                let (line, next) = take_line(rest, width);
                lines.push(line);
                rest = next;
            }
        }
    }
    lines
}

fn take_line(text: &str, width: usize) -> (String, &str) {
    let mut used = 0usize;
    let mut end = 0usize;
    let mut last_space = None;
    for (index, ch) in text.char_indices() {
        let char_cols = usize::from(char_width(ch));
        if used + char_cols > width {
            break;
        }
        used += char_cols;
        end = index + ch.len_utf8();
        if ch == ' ' {
            last_space = Some(end);
        }
    }
    if end == 0 {
        let skip = text
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(text.len());
        (text[..skip].to_owned(), &text[skip..])
    } else if end < text.len() {
        if let Some(space) = last_space {
            (
                text[..space].trim_end().to_owned(),
                text[space..].trim_start(),
            )
        } else {
            (text[..end].to_owned(), text[end..].trim_start())
        }
    } else {
        (text.to_owned(), "")
    }
}

fn visible_start(text: &str, cursor: usize, width: usize) -> usize {
    let cursor = cursor.min(text.chars().count());
    let mut start = 0usize;
    while start < cursor && prefix_width(text, cursor) - prefix_width(text, start) >= width {
        start += 1;
    }
    start
}

fn char_index_at(text: &str, columns: usize) -> usize {
    let mut used = 0usize;
    let mut count = 0usize;
    for ch in text.chars() {
        let width = usize::from(char_width(ch));
        if used + width > columns {
            break;
        }
        used += width;
        count += 1;
    }
    count
}

fn prefix_width(text: &str, chars: usize) -> usize {
    text.chars()
        .take(chars)
        .map(|ch| usize::from(char_width(ch)))
        .sum()
}

fn byte_at(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf8).sum()
}
