use log::error;
use thiserror::Error;

use crate::graphics::Color;

/// A color as used by ansi control sequences. This can either be an
/// index into one of the color maps or a True [crate::graphics::Color].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TextColor {
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

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum AnsiSGR {
    /// reset the style back to the default
    Reset,
    /// make the style bold
    Bold,
    /// make the style faint
    Faint,
    /// underline the text
    Underline,
    /// change the cursor blink speed to slow
    SlowBlink,
    /// set the foreground color
    Foreground(TextColor),
    /// set the background color
    Background(TextColor),
}

/// enum for all sgr parsing related errors
#[derive(Error, Debug, PartialEq, Eq, Clone, Copy)]
#[allow(missing_docs)]
pub enum SGRParseError {
    #[error("input is not long enough")]
    EOF,
    #[error("expected '{0}' but got '{1}'")]
    ExpectedChar(char, char),
    #[error("expected digit but got '{0}'")]
    ExpectedDigit(char),
    #[error("expected digit or separator(';') but got '{0}'")]
    ExpectedDigitOrSep(char),
    #[error("expected digit or end('m') but got '{0}'")]
    ExpectedDigitOrEnd(char),
    #[error("expected digit, separator(';') or end('m') but got '{0}'")]
    ExpectedDigitOrSepOrEnd(char),
    #[error("expected end('m') but got '{0}'")]
    ExpectedEnd(char),
    #[error("the mode {0} requires 1 or more parameters")]
    ModeMissingParameters(u8),
    #[error("invalid mode {0}")]
    InvalidMode(u8),
    #[error("invalid color mode {0}")]
    InvalidColorMode(u8),
}

enum SGRParseState {
    Start,
    Esc,
    ParseMode(u8),
    ParseFg(ColorParseState),
    ParseBg(ColorParseState),
    Err(SGRParseError),
    Done(AnsiSGR),
}

enum ColorParseState {
    ParseMode,
    Extended(u8),
    R(u8),
    G(u8, u8),
    B(u8, u8, u8),
    Done(TextColor),
    Err(SGRParseError),
}

impl SGRParseState {
    fn next(&mut self, c: char) {
        match self {
            SGRParseState::Start => self.expect_char(c, '\x1b', SGRParseState::Esc),
            SGRParseState::Esc => self.expect_char(c, '[', SGRParseState::ParseMode(0)),
            SGRParseState::ParseMode(mode) => {
                let mode = *mode;
                self.parse_mode_next(c, mode)
            }
            SGRParseState::Err(e) => panic!("calling next on \"Err\" state: {e}"),
            SGRParseState::Done(_) => error!("calling next on \"Done\" state"),
            SGRParseState::ParseFg(state) => {
                Self::parse_color(c, state);
                match state {
                    ColorParseState::Done(color) => {
                        *self = SGRParseState::Done(AnsiSGR::Foreground(*color))
                    }
                    ColorParseState::Err(e) => *self = SGRParseState::Err(*e),
                    _ => {}
                }
            }
            SGRParseState::ParseBg(state) => {
                Self::parse_color(c, state);
                match state {
                    ColorParseState::Done(color) => {
                        *self = SGRParseState::Done(AnsiSGR::Background(*color))
                    }
                    ColorParseState::Err(e) => *self = SGRParseState::Err(*e),
                    _ => {}
                }
            }
        }
    }

    fn parse_color(c: char, state: &mut ColorParseState) {
        match state {
            ColorParseState::ParseMode => match c {
                '0'..='9' => {
                    let digit = c as u8 - b'0';
                    match digit {
                        2 => *state = ColorParseState::Extended(0),
                        5 => *state = ColorParseState::R(0),
                        _ => *state = ColorParseState::Err(SGRParseError::InvalidColorMode(digit)),
                    }
                }
                _ => *state = ColorParseState::Err(SGRParseError::ExpectedDigit(c)),
            },
            ColorParseState::R(r) => match c {
                ';' => *state = ColorParseState::G(*r, 0),
                '0'..='9' => {
                    let d = c as u8 - b'0';
                    *state = ColorParseState::R(*r * 10 + d)
                }
                _ => *state = ColorParseState::Err(SGRParseError::ExpectedDigitOrSep(c)),
            },
            ColorParseState::G(r, g) => match c {
                ';' => *state = ColorParseState::B(*r, *g, 0),
                '0'..='9' => {
                    let d = c as u8 - b'0';
                    *state = ColorParseState::G(*r, *g * 10 + d)
                }
                _ => *state = ColorParseState::Err(SGRParseError::ExpectedDigitOrSep(c)),
            },
            ColorParseState::B(r, g, b) => match c {
                'm' => {
                    let color = Color::new(*r, *g, *b);
                    let color = TextColor::True(color);
                    *state = ColorParseState::Done(color);
                }
                '0'..='9' => {
                    let d = c as u8 - b'0';
                    *state = ColorParseState::B(*r, *g, *b * 10 + d)
                }
                _ => *state = ColorParseState::Err(SGRParseError::ExpectedDigitOrEnd(c)),
            },
            ColorParseState::Extended(v) => match c {
                'm' => {
                    let color = TextColor::Extended(*v);
                    *state = ColorParseState::Done(color);
                }
                '0'..='9' => {
                    let d = c as u8 - b'0';
                    *state = ColorParseState::Extended(*v * 10 + d);
                }
                _ => *state = ColorParseState::Err(SGRParseError::ExpectedDigitOrEnd(c)),
            },
            ColorParseState::Err(e) => panic!("calling next on \"Err\" state: {e}"),
            ColorParseState::Done(_) => error!("calling next on \"Done\" state"),
        }
    }

    fn parse_mode_next(&mut self, c: char, mode: u8) {
        match c {
            ';' => match mode {
                38 => *self = SGRParseState::ParseFg(ColorParseState::ParseMode),
                48 => *self = SGRParseState::ParseBg(ColorParseState::ParseMode),
                0..=2 | 4 | 5 | 30..=37 | 40..=47 | 90..=97 | 100..=107 => {
                    *self = SGRParseState::Err(SGRParseError::ExpectedEnd(c))
                }
                _ => *self = SGRParseState::Err(SGRParseError::InvalidMode(mode)),
            },
            'm' => match mode {
                0..=2 | 4 | 5 => {
                    *self = SGRParseState::Done(AnsiSGR::mode(mode));
                }
                30..=37 => {
                    let color = mode - 30;
                    let color = TextColor::Normal(color);
                    *self = SGRParseState::Done(AnsiSGR::Foreground(color));
                }
                38 => {
                    *self = SGRParseState::Err(SGRParseError::ModeMissingParameters(mode));
                }
                40..=47 => {
                    let color = mode - 40;
                    let color = TextColor::Normal(color);
                    *self = SGRParseState::Done(AnsiSGR::Background(color));
                }
                48 => {
                    *self = SGRParseState::Err(SGRParseError::ModeMissingParameters(mode));
                }
                90..=97 => {
                    let color = mode - 90;
                    let color = TextColor::Bright(color);
                    *self = SGRParseState::Done(AnsiSGR::Foreground(color));
                }
                100..=107 => {
                    let color = mode - 100;
                    let color = TextColor::Bright(color);
                    *self = SGRParseState::Done(AnsiSGR::Foreground(color));
                }
                _ => {
                    *self = SGRParseState::Err(SGRParseError::InvalidMode(mode));
                }
            },
            '0'..='9' => {
                let d = c as u8 - b'0';
                let mode = mode * 10 + d;
                *self = SGRParseState::ParseMode(mode);
            }
            _ => {
                *self = SGRParseState::Err(SGRParseError::ExpectedDigitOrSepOrEnd(c));
            }
        }
    }

    fn expect_char(&mut self, current: char, expected: char, next: SGRParseState) {
        if current != expected {
            *self = SGRParseState::Err(SGRParseError::ExpectedChar(expected, current));
            return;
        }
        *self = next;
    }

    fn read_digit(&mut self, current: char) -> Result<u8, SGRParseError> {
        if !('0'..='9').contains(&current) {
            Err(SGRParseError::ExpectedDigit(current))
        } else {
            current
                .try_into()
                .map_err(|_| SGRParseError::ExpectedDigit(current))
                .map(|c: u8| c - b'0')
        }
    }
}

impl AnsiSGR {
    /// Parse AnsiSGR (Select Graphic Rendition) from char iterator.
    ///
    /// if `no_esc` is set this assumes that the ESC char in the SGR was already
    /// read and the next char is expected to be a `[`.
    /// Otherwise the first to chars are expected to be `\0x1b[`.
    ///
    /// See: <https://chrisyeh96.github.io/2020/03/28/terminal-colors.html>
    pub fn parse_from_chars(
        chars: &mut impl Iterator<Item = char>,
        no_esc: bool,
    ) -> Result<AnsiSGR, SGRParseError> {
        let mut parse_state = if no_esc {
            SGRParseState::Esc
        } else {
            SGRParseState::Start
        };

        loop {
            let Some(c) = chars.next() else {
                return Err(SGRParseError::EOF);
            };
            parse_state.next(c);
            match parse_state {
                SGRParseState::Done(sgr) => return Ok(sgr),
                SGRParseState::Err(e) => return Err(e),
                _ => (),
            }
        }
    }

    fn mode(mode: u8) -> AnsiSGR {
        match mode {
            0 => AnsiSGR::Reset,
            1 => AnsiSGR::Bold,
            2 => AnsiSGR::Faint,
            4 => AnsiSGR::Underline,
            5 => AnsiSGR::SlowBlink,
            _ => panic!("unexpected mode"),
        }
    }
}
