//! Widgets for a terminal screen that is drawn again each frame.
//!
//! The application holds the state. Each frame fills a [`Buffer`], and a
//! backend writes the cells that changed. This crate does not know about
//! Bitbucket or bistill.

#![deny(unsafe_code)]

mod backend;
mod buffer;
mod layout;
mod style;
mod terminal;
mod widgets;
mod width;

#[cfg(unix)]
#[path = "terminal_unix.rs"]
mod terminal_os;
#[cfg(windows)]
#[path = "terminal_windows.rs"]
mod terminal_os;

pub use backend::{Backend, Event, KeyCode, MouseButton, TestBackend, Wheel};
pub use buffer::{Buffer, Cell, Glyph, Line, Span};
pub use layout::{Constraint, Direction, Rect, split};
pub use style::{Color, Style};
pub use terminal::Terminal;
pub use widgets::{
    Input, ListState, draw_block, draw_input, draw_list, draw_paragraph, draw_table, draw_tabs,
    ensure_visible, fill_rect, hit_row, inner, position,
};
pub use width::{char_width, display_width, truncate};

#[cfg(test)]
mod tests;
