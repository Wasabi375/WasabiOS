//! A Canvas represents a surface that can be drawn to.
//!
//! This can be a framebuffer, a image or anything else.
use alloc::format;
use core::fmt::Write;
use derive_builder::Builder;
use log::error;
use thiserror::Error;

use super::{
    kernel_font::BitFont,
    tty::{self, AnsiSGR},
    Color, Point,
};

/// A surface which can be drawn on. Screen, Screen region, etc
pub trait Canvas {
    /// cleras the canvas to `color`
    fn clear(&mut self, c: Color);

    /// sets the pixel at `(x, y)` to `color`
    fn set_pixel(&mut self, x: u32, y: u32, color: Color);

    /// the width in pixel
    fn width(&self) -> u32;

    /// the height in pixel
    fn height(&self) -> u32;

    /// returns true if this implementation of Canvas supports scrolling
    fn supports_scrolling() -> bool;

    /// scrolls the canvas by `height` pixels
    ///
    /// a positive `height` means that every row of pixels is moved up by
    /// `height` pixels and the bottom rows of pixels are cleard to `clear_color`.
    fn scroll(
        &mut self,
        height: i32,
        clear_color: Color,
    ) -> core::result::Result<(), ScrollingNotSupportedError>;
}

/// Scrolling is not supported
///
/// returned by scrolling actions in a canvas if scrolling is not supported
/// by the canvas
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
pub struct ScrollingNotSupportedError;

impl core::fmt::Display for ScrollingNotSupportedError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Scrolling is not supported")
    }
}

/// enum for all canvas writer related errors
#[derive(Error, Debug, PartialEq, Eq, Clone, Copy)]
#[allow(missing_docs)]
pub enum CanvasWriterError {
    #[error("scrolling is not supported for this canvas writer")]
    ScrollingNotSupported(ScrollingNotSupportedError),
    #[error("failed to parse ansi control sequence: {0}")]
    SGRParsing(tty::SGRParseError),
    #[error("feature {0} is not implemented")]
    Todo(&'static str),
    #[error("Color pallet not supported for {0}")]
    ColorError(tty::TextColorError),
}

impl From<tty::SGRParseError> for CanvasWriterError {
    fn from(value: tty::SGRParseError) -> Self {
        CanvasWriterError::SGRParsing(value)
    }
}

impl From<tty::TextColorError> for CanvasWriterError {
    fn from(value: tty::TextColorError) -> Self {
        CanvasWriterError::ColorError(value)
    }
}

/// A [Write]r for a [Canvas]
#[derive(Debug, Builder)]
#[builder(
    no_std,
    pattern = "owned",
    build_fn(validate = "Self::validate", error = "CanvasWriterBuilderError")
)]
//#[doc = "A Builder for a [CanvasWriter]"]
pub struct CanvasWriter<C: Canvas> {
    /// the [Canvas] to write to
    canvas: C,

    /// the [BitFont] used for the text
    font: BitFont,

    /// how much to indent the next line.
    #[builder(default = "0")]
    pub ident_line: u32,

    /// horizontal border to the canvas edge in pixel
    #[builder(default = "0")]
    border_width: u32,

    /// vertical border to the canvas edge in pixel
    #[builder(default = "0")]
    border_height: u32,

    /// initial position of the cursor
    #[builder(default = "self._build_cursor()", setter(skip))]
    cursor: Point,

    /// the active text color
    #[builder(
        default = "self.default_text_color.unwrap_or(tty::color::DEFAULT_TEXT)",
        setter(skip)
    )]
    text_color: Color,

    /// the default text color
    #[builder(default = "tty::color::DEFAULT_TEXT", setter(name = "text_color"))]
    default_text_color: Color,

    /// the active background color
    #[builder(
        default = "self.default_text_color.unwrap_or(tty::color::DEFAULT_BACKGROUND)",
        setter(skip)
    )]
    background_color: Color,

    /// the default text color
    #[builder(
        default = "tty::color::DEFAULT_BACKGROUND",
        setter(name = "background_color")
    )]
    default_background_color: Color,

    /// the default scroll beahviour of the writer
    #[builder(default)]
    scroll_behaviour: CanvasWriterScrollBehaviour,

    /// logs errors if set to `true`.
    ///
    /// If set to `false` if `write_str` fails with [core::fmt::Error] there
    /// is no way to get the reason of the failure.
    ///
    /// This is usefull if the [CanvasWriter] is used as the target for a logger.
    #[builder(default = "true")]
    log_errors: bool,

    /// if set the writer will ignore all ansi control sequences
    #[builder(default = "false")]
    ignore_ansi: bool,

    /// size of a single tab stop
    #[builder(default = "4")]
    tab_size: u32,
}

/// Error used by [CanvasWriterBuilder]
#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum CanvasWriterBuilderError {
    UninitializedField(&'static str),
    ScrollingNotSupported,
}

impl From<derive_builder::UninitializedFieldError> for CanvasWriterBuilderError {
    fn from(value: derive_builder::UninitializedFieldError) -> Self {
        Self::UninitializedField(value.field_name())
    }
}

impl core::fmt::Display for CanvasWriterBuilderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CanvasWriterBuilderError::UninitializedField(field) => f.write_str(&format!(
                "Field  \"{}\" not initialized in CanvasWriterBuilder",
                field
            )),
            CanvasWriterBuilderError::ScrollingNotSupported => {
                f.write_str("Scrolling not supported by Canvas used in CanvasWriterBuilder")
            }
        }
    }
}

/// The Scroll behaviour of a [CanvasWriter]
#[derive(Debug, Copy, Clone)]
pub enum CanvasWriterScrollBehaviour {
    /// The canvas scrolls, when the end of the area is reached.
    ///
    /// When a new line is written and the canvas is "full" the canvas
    /// scrolls down by 1 line.
    Scroll,
    /// The canvas is cleared, when the end of the area is reached.
    ///
    /// When a new line is written and the canvas is "full" the canvas is
    /// cleared and the new line is written from the top.
    #[allow(dead_code)]
    Clear,
}

impl Default for CanvasWriterScrollBehaviour {
    fn default() -> Self {
        CanvasWriterScrollBehaviour::Scroll
    }
}

impl<C> CanvasWriterBuilder<C>
where
    C: Canvas,
{
    #[doc(hidden)]
    fn _build_cursor(&self) -> Point {
        let b_width = self.border_width.unwrap_or(0);
        let b_height = self.border_height.unwrap_or(0);
        let indent = self.ident_line.unwrap_or(0);

        Point {
            x: b_width + indent,
            y: b_height,
        }
    }

    fn validate(&self) -> Result<(), CanvasWriterBuilderError> {
        match self.scroll_behaviour.unwrap_or(Default::default()) {
            CanvasWriterScrollBehaviour::Scroll => {
                if C::supports_scrolling() {
                    Ok(())
                } else {
                    Err(CanvasWriterBuilderError::ScrollingNotSupported)
                }
            }
            CanvasWriterScrollBehaviour::Clear => Ok(()),
        }
    }
}

impl<C> CanvasWriter<C>
where
    C: Canvas,
{
    /// creates a [CanvasWriterBuilder]
    pub fn builder() -> CanvasWriterBuilder<C> {
        CanvasWriterBuilder::create_empty()
    }

    /// returns the internally used [Canvas]
    pub fn into_canvas(self) -> C {
        self.canvas
    }
}

impl<C: Canvas> CanvasWriter<C> {
    /// jump to the next line
    pub fn new_line(&mut self) {
        self.carriage_return();
        if self.cursor.y + self.font.line_height() > self.canvas.height() - self.border_height {
            use CanvasWriterScrollBehaviour as CWSB;
            match self.scroll_behaviour {
                CWSB::Scroll => {
                    self.canvas
                        .scroll(self.font.line_height() as i32, self.background_color)
                        .expect("The builder should have checked that scrolling is supported");
                }
                CWSB::Clear => {
                    self.canvas.clear(self.background_color);
                    self.cursor.y = self.border_height;
                }
            }
        } else {
            self.cursor.y += self.font.line_height();
        }
    }

    /// jump back to the start of the current line
    #[inline]
    pub fn carriage_return(&mut self) {
        self.cursor.x = self.border_width + self.ident_line;
    }

    /// advances the cursor by 1 character.
    ///
    /// This done automatically when calling [print_char]
    #[inline]
    pub fn tab_stop(&mut self) {
        self.cursor.x += self.font.char_width() * self.tab_size;
        if self.cursor.x >= self.canvas.width() - self.border_width {
            self.new_line();
        }
    }

    /// advances the cursor by 1 character.
    ///
    /// This done automatically when calling [print_char]
    #[inline]
    pub fn advance_cursor(&mut self) {
        self.cursor.x += self.font.char_width();
        if self.cursor.x >= self.canvas.width() - self.border_width {
            self.new_line();
        }
    }

    /// write a single character to the screen
    pub fn print_char(&mut self, c: char) {
        // print char to pos
        self.font.draw_char(
            c,
            self.cursor,
            self.text_color,
            self.background_color,
            &mut self.canvas,
        );

        self.advance_cursor();
    }

    #[cfg(feature = "no-color")]
    fn handle_ansi_ctrl_seq(
        &mut self,
        chars: &mut impl Iterator<Item = char>,
    ) -> Result<(), CanvasWriterError> {
        // skip ansi sgr sequence and ignore possible errors
        let _ = AnsiSGR::parse_from_chars(chars, true);
        Ok(())
    }

    #[cfg(not(feature = "no-color"))]
    /// Handles ansi colors
    ///
    /// `chars` should be the rest of the ansi control sequence fater the `ESC(0x1b)`.
    ///
    /// A sequence looks like `ESC[(0-9){1,3}(;(0-9){1,3})*m`
    /// <https://chrisyeh96.github.io/2020/03/28/terminal-colors.html>
    fn handle_ansi_ctrl_seq(
        &mut self,
        chars: &mut impl Iterator<Item = char>,
    ) -> Result<(), CanvasWriterError> {
        let sgr = AnsiSGR::parse_from_chars(chars, true)
            .map_err(|e| Into::<CanvasWriterError>::into(e))?;
        if self.ignore_ansi {
            return Ok(());
        }
        match sgr {
            AnsiSGR::Reset => self.reset_to_defaults(),
            AnsiSGR::Bold => return Err(CanvasWriterError::Todo("bold text")),
            AnsiSGR::Faint => return Err(CanvasWriterError::Todo("faint text")),
            AnsiSGR::Underline => return Err(CanvasWriterError::Todo("underlined text")),
            AnsiSGR::SlowBlink => return Err(CanvasWriterError::Todo("underlined text")),
            AnsiSGR::Foreground(c) => self.text_color = c.try_into()?,
            AnsiSGR::Background(c) => self.background_color = c.try_into()?,
        }

        Ok(())
    }

    /// resets the [CanvasWriter] to it's default values.
    ///
    /// This effects all style values. Currently:
    ///  * [text_color]
    ///  * [background_color]
    pub fn reset_to_defaults(&mut self) {
        self.text_color = self.default_text_color;
        self.background_color = self.default_background_color;
    }
}

impl<C: Canvas> Write for CanvasWriter<C> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '\n' => self.new_line(),
                '\r' => self.carriage_return(),
                '\t' => self.tab_stop(),
                '\x1b' => self.handle_ansi_ctrl_seq(&mut chars).map_err(|e| {
                    if self.log_errors {
                        error!("Failed to write to canvas: {e}")
                    }
                    core::fmt::Error
                })?,
                c => self.print_char(c),
            }
        }
        Ok(())
    }
}
