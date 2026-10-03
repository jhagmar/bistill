//! Immediate-mode terminal widgets.
//!
//! The application keeps the state. Each frame draws a [`Buffer`] and a
//! backend presents the cells that changed. This crate has no Bistill types.

#![deny(unsafe_code)]

mod backend;
mod buffer;
mod layout;
mod style;
mod widgets;
mod width;

pub use backend::{Backend, Event, KeyCode, MouseButton, TestBackend, Wheel};
pub use buffer::{Buffer, Cell, Glyph, Line, Span};
pub use layout::{Constraint, Direction, Rect, split};
pub use style::{Color, Style};
pub use widgets::{
    Input, ListState, draw_block, draw_input, draw_list, draw_paragraph, draw_table, draw_tabs,
    ensure_visible, hit_row, inner, position,
};
pub use width::{char_width, display_width, truncate};

#[cfg(test)]
mod tests;
