//! Text colors for terminal
//!
//! The defaults are taken from
//! <https://github.com/alacritty/alacritty-theme/blob/11664caf4c2bfae94e006e860042f93b217fc0f7/themes/blood_moon.toml>
use thiserror::Error;

use crate::graphics::Color;

/// A color as used by ansi control sequences. This can either be an
/// index into one of the color maps or a True [crate::graphics::Color].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TextColor {
    /// The default color
    Default,
    /// The default background color
    DefaultBackground,
    /// Index into the normal color map.
    ///
    /// Only 1-8 are valid
    Normal(u8),
    /// Index into the bright color map.
    ///
    /// Only 1-8 are valid
    Bright(u8),
    /// Index into the extended color map.
    ///
    /// This can map the entire u8 range.
    Extended(u8),
    /// A true rgb color using u8 for each channel
    True(Color),
}

impl TryInto<Color> for TextColor {
    type Error = TextColorError;

    fn try_into(self) -> Result<Color, Self::Error> {
        match self {
            TextColor::Default => Ok(DEFAULT_TEXT),
            TextColor::DefaultBackground => Ok(DEFAULT_BACKGROUND),
            TextColor::Normal(i) if i < 8 => Ok(NORMAL_COLORS[i as usize]),
            TextColor::Normal(i) => Err(TextColorError::IndexOutOfBounds(i, 8)),
            TextColor::Bright(i) if i < 8 => Ok(BRIGHT_COLORS[i as usize]),
            TextColor::Bright(i) => Err(TextColorError::IndexOutOfBounds(i, 8)),
            TextColor::Extended(_) => Err(TextColorError::NotSupported(self)),
            TextColor::True(color) => Ok(color),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
#[allow(missing_docs)]
pub enum TextColorError {
    #[error("Text color {0:?} not supported")]
    NotSupported(TextColor),
    #[error("Color index out of bounds. Index: {0}, Max: {1}")]
    IndexOutOfBounds(u8, u8),
}

/// default text color
pub const DEFAULT_TEXT: Color = Color::new(0xc6, 0xc6, 0xc4);
/// default background color
pub const DEFAULT_BACKGROUND: Color = Color::new(0x10, 0x10, 0x0e);

/// normal colors
///
/// Orderd of colors is Blac, Red, Green, Yellow, Blue, Magenta, Cyan, White
pub const NORMAL_COLORS: [Color; 8] = [
    Color::new(0x10, 0x10, 0x0e), // Black
    Color::new(0xc4, 0x02, 0x33), // Red
    Color::new(0x00, 0x9f, 0x6b), // Green
    Color::new(0xff, 0xd7, 0x00), // Yellow
    Color::new(0xff, 0x87, 0xbd), // Blue
    Color::new(0x9a, 0x4e, 0xae), // Magenta
    Color::new(0x20, 0xb2, 0xaa), // Cyan
    Color::new(0xC6, 0xC6, 0xC4), // White
];

/// bright colors
///
/// Orderd of colors is Blac, Red, Green, Yellow, Blue, Magenta, Cyan, White
pub const BRIGHT_COLORS: [Color; 8] = [
    Color::new(0x69, 0x69, 0x69), // Black
    Color::new(0xFF, 0x24, 0x00), // Red
    Color::new(0x03, 0xC0, 0x3C), // Green
    Color::new(0xFD, 0xFF, 0x00), // Yellow
    Color::new(0x00, 0x7F, 0xFF), // Blue
    Color::new(0xFF, 0x14, 0x93), // Magenta
    Color::new(0x00, 0xCC, 0xCC), // Cyan
    Color::new(0xFF, 0xFA, 0xFA), // White
];
