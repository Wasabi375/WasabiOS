//! Module containing all graphics related code for the kernel

pub mod fb;
pub mod kernel_font;

use core::{fmt::Write, slice};

use derive_builder::Builder;
pub use fb::framebuffer;

use self::{
    fb::{
        startup::{take_boot_framebuffer, HARDWARE_FRAMEBUFFER_START_INFO},
        Framebuffer,
    },
    kernel_font::BitFont,
};

/// A simple rgb (u8) Color
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[allow(missing_docs)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[allow(missing_docs)]
impl Color {
    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
    };
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };
    pub const RED: Color = Color { r: 255, g: 0, b: 0 };
    pub const GREEN: Color = Color { r: 0, g: 255, b: 0 };
    pub const BLUE: Color = Color { r: 0, g: 0, b: 255 };
    pub const PINK: Color = Color {
        r: 255,
        g: 0,
        b: 255,
    };

    pub const PANIC: Color = Color::PINK;
}

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
#[builder(no_std)]
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
    pub fn new_line(&mut self) {
        self.carriage_return();
        if self.cursor.y + self.font.line_height() > self.canvas.height() - self.border_height {
            todo!("scroll or clear");
        } else {
            self.cursor.y += self.font.line_height();
        }
    }

    /// jump back to the start of the current line
    pub fn carriage_return(&mut self) {
        self.cursor.x = self.border_width + self.ident_line;
    }

    /// write a single character to the screen
    pub fn print_char(&mut self, _c: char) {
        todo!()
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
pub unsafe fn init() {
    let fb: Framebuffer = unsafe {
        take_boot_framebuffer()
            .expect("No framebuffer found")
            .into()
    };

    unsafe {
        HARDWARE_FRAMEBUFFER_START_INFO = Some((fb.start, fb.info.clone()));
    }

    framebuffer().lock_uninit().write(fb);
}
