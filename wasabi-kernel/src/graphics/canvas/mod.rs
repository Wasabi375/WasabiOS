use alloc::format;
use core::fmt::Write;
use thiserror::Error;

use derive_builder::Builder;

use super::{kernel_font::BitFont, Color, Point};

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
#[derive(Debug, Error)]
pub struct ScrollingNotSupportedError;

impl core::fmt::Display for ScrollingNotSupportedError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Scrolling is not supported")
    }
}

/// A [Write]r for a [Canvas]
#[derive(Debug, Builder)]
#[builder(
    no_std,
    pattern = "owned",
    build_fn(validate = "Self::validate", error = "CanvasWriterBuilderError")
)]
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

    /// the default text col."or
    #[builder(default = "Color::WHITE")]
    text_color: Color,

    /// the default text color
    #[builder(default = "Color::BLACK")]
    background_color: Color,

    #[builder(default)]
    scroll_behaviour: CanvasWriterScrollBehaviour,
}

#[derive(Debug, Error)]
pub enum CanvasWriterBuilderError {
    UninitializedFiled(&'static str),
    ScrollingNotSupported,
}

impl From<derive_builder::UninitializedFieldError> for CanvasWriterBuilderError {
    fn from(value: derive_builder::UninitializedFieldError) -> Self {
        Self::UninitializedFiled(value.field_name())
    }
}

impl core::fmt::Display for CanvasWriterBuilderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CanvasWriterBuilderError::UninitializedFiled(field) => f.write_str(&format!(
                "Field  \"{}\" not initialized in CanvasWriterBuilder",
                field
            )),
            CanvasWriterBuilderError::ScrollingNotSupported => {
                f.write_str("Scrolling not supported by Canvas used in CanvasWriterBuilder")
            }
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum CanvasWriterScrollBehaviour {
    Scroll,
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
    /// creates a [CanvasWrtierBuilder]
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
}

impl<C: Canvas> Write for CanvasWriter<C> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            match c {
                '\n' => self.new_line(),
                '\r' => self.carriage_return(),
                c => self.print_char(c),
            }
        }
        Ok(())
    }
}
