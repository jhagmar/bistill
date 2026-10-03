//! The 16 ANSI colors and the attributes a cell can carry.

/// One of the 16 ANSI colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Color {
    /// ANSI black.
    Black,
    /// ANSI red.
    Red,
    /// ANSI green.
    Green,
    /// ANSI yellow.
    Yellow,
    /// ANSI blue.
    Blue,
    /// ANSI magenta.
    Magenta,
    /// ANSI cyan.
    Cyan,
    /// ANSI white.
    White,
    /// ANSI bright black.
    BrightBlack,
    /// ANSI bright red.
    BrightRed,
    /// ANSI bright green.
    BrightGreen,
    /// ANSI bright yellow.
    BrightYellow,
    /// ANSI bright blue.
    BrightBlue,
    /// ANSI bright magenta.
    BrightMagenta,
    /// ANSI bright cyan.
    BrightCyan,
    /// ANSI bright white.
    BrightWhite,
}

impl Color {
    /// Every color, in ANSI order.
    pub const ALL: [Color; 16] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::White,
        Color::BrightBlack,
        Color::BrightRed,
        Color::BrightGreen,
        Color::BrightYellow,
        Color::BrightBlue,
        Color::BrightMagenta,
        Color::BrightCyan,
        Color::BrightWhite,
    ];

    /// The ANSI index of this color, from 0 through 15.
    pub fn ansi(self) -> u8 {
        match self {
            Color::Black => 0,
            Color::Red => 1,
            Color::Green => 2,
            Color::Yellow => 3,
            Color::Blue => 4,
            Color::Magenta => 5,
            Color::Cyan => 6,
            Color::White => 7,
            Color::BrightBlack => 8,
            Color::BrightRed => 9,
            Color::BrightGreen => 10,
            Color::BrightYellow => 11,
            Color::BrightBlue => 12,
            Color::BrightMagenta => 13,
            Color::BrightCyan => 14,
            Color::BrightWhite => 15,
        }
    }
}

/// Foreground, background, and text attributes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Style {
    /// Foreground color.
    pub fg: Option<Color>,
    /// Background color.
    pub bg: Option<Color>,
    /// Bold text.
    pub bold: bool,
    /// Underlined text.
    pub underline: bool,
    /// Foreground and background exchanged.
    pub reverse: bool,
}
