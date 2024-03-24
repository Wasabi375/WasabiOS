//! Module containing all graphics related code for the kernel

mod color;
pub mod fb;
pub mod kernel_font;
pub use color::Color;
use log::warn;
use shared::lockcell::LockCell;

use core::{fmt::Write, slice};

use derive_builder::Builder;

use self::{
    fb::{
        startup::{take_boot_framebuffer, HARDWARE_FRAMEBUFFER_START_INFO},
        Framebuffer, HARDWARE_FRAMEBUFFER,
    },
    kernel_font::BitFont,
};

/// A point in 2D space
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default)]
#[repr(C)]
#[allow(missing_docs)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

impl Point {
    /// creates a new 2d point
    pub fn new(x: u32, y: u32) -> Self {
        Point { x, y }
    }
}

impl AsRef<[u32]> for Point {
    fn as_ref(&self) -> &[u32] {
        unsafe { slice::from_raw_parts(&self.x as *const u32, 2) }
    }
}

impl AsMut<[u32]> for Point {
    fn as_mut(&mut self) -> &mut [u32] {
        unsafe { slice::from_raw_parts_mut(&mut self.x as *mut u32, 2) }
    }
}

/// A surface which can be drawn on. Screen, Screen region, etc
pub trait Canvas {
    /// clears the canvas to `color`
    fn clear(&mut self, c: Color);

    /// sets the pixel at `(x, y)` to `color`
    fn set_pixel(&mut self, x: u32, y: u32, color: Color);

    /// the width in pixel
    fn width(&self) -> u32;

    /// the height in pixel
    fn height(&self) -> u32;
}

/// A [Write]r for a [Canvas]
#[derive(Debug, Builder)]
#[builder(
    no_std,
    pattern = "owned",
    build_fn(error = "derive_builder::UninitializedFieldError")
)]
pub struct CanvasWriter<C> {
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

    /// the default text color
    #[builder(default = "Color::WHITE")]
    text_color: Color,

    /// the default text color
    #[builder(default = "Color::BLACK")]
    background_color: Color,
}

impl<C> CanvasWriterBuilder<C> {
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
}

impl<C> CanvasWriter<C> {
    /// returns the internally used [Canvas]
    pub fn into_canvas(self) -> C {
        self.canvas
    }
}

impl<C: Canvas> CanvasWriter<C> {
    /// jump to the next line
    #[inline]
    pub fn new_line(&mut self) {
        self.carriage_return();
        if self.cursor.y + self.font.line_height() > self.canvas.height() - self.border_height {
            todo!("scroll or clear");
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

/// Initializes graphics
///
/// # Safety
///
/// must only be called once during initialization.
/// Requires logging and heap access.
pub unsafe fn init(frambuffer_logger: bool) {
    let fb: Framebuffer = unsafe {
        take_boot_framebuffer()
            .expect("No framebuffer found")
            .into()
    };

    unsafe {
        HARDWARE_FRAMEBUFFER_START_INFO = Some((fb.start, fb.info.clone()));
    }

    *HARDWARE_FRAMEBUFFER.lock() = Some(fb);

    if frambuffer_logger {
        init_framebuffer_logger();
    }
}

fn init_framebuffer_logger() {
    let fb = if let Some(fb) = HARDWARE_FRAMEBUFFER.lock().take() {
        fb
    } else {
        warn!("Framebuffer already taken. Frambuffer logger will not be created");
        return;
    };

    let canvas_writer = CanvasWriterBuilder::create_empty()
        .font(todo!("kernel font"))
        .canvas(fb)
        .border_width(10)
        .border_height(10)
        .build()
        .expect("Canvas writer should be fully initialized");
}
