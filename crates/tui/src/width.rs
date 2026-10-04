//! How many columns a piece of Unicode text takes on screen.

#[path = "width_table.rs"]
mod width_table;

pub(crate) use width_table::{COMBINING, WIDE};

/// Column width of one scalar value.
///
/// A combining mark (general category Mn or Me) is 0. An East Asian Wide or
/// Fullwidth scalar is 2. Every other scalar is 1.
pub fn char_width(ch: char) -> u8 {
    let cp = ch as u32;
    if in_ranges(cp, COMBINING) {
        0
    } else if in_ranges(cp, WIDE) {
        2
    } else {
        1
    }
}

/// Column width of `text`, summing [`char_width`].
pub fn display_width(text: &str) -> usize {
    text.chars().map(|ch| usize::from(char_width(ch))).sum()
}

/// `text` clipped to `columns`, with an ellipsis when clipping drops scalars.
///
/// A `columns` of 0 yields an empty string. The ellipsis is U+2026.
pub fn truncate(text: &str, columns: usize) -> String {
    if display_width(text) <= columns {
        text.to_owned()
    } else {
        cut(text, columns)
    }
}

fn cut(text: &str, columns: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    let budget = columns.saturating_sub(1);
    for ch in text.chars() {
        let width = usize::from(char_width(ch));
        if used + width > budget {
            break;
        }
        out.push(ch);
        used += width;
    }
    if columns > 0 {
        out.push('\u{2026}');
    }
    out
}

fn in_ranges(cp: u32, ranges: &[(u32, u32)]) -> bool {
    let mut lo = 0;
    let mut hi = ranges.len();
    let mut found = false;
    while lo < hi && !found {
        let mid = lo + (hi - lo) / 2;
        let (start, end) = ranges[mid];
        if cp < start {
            hi = mid;
        } else if cp > end {
            lo = mid + 1;
        } else {
            found = true;
        }
    }
    found
}
