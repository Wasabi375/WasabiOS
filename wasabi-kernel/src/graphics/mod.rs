//! Module containing all graphics related code for the kernel

pub mod fb;
pub mod kernel_font;

pub use fb::framebuffer;

use self::fb::{Framebuffer, HARDWARE_FRAMEBUFFER_START};
use crate::boot_info;
use bootloader_api::info::{FrameBuffer as BootFrameBuffer, Optional};

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

/// A surface which can be drawn on. Screen, Screen region, etc
pub trait Canvas {
    /// clears the canvas to `color`
    fn clear(&mut self, c: Color);

    /// sets the pixel at `(x, y)` to `color`
    fn set_pixel(&mut self, x: u32, y: u32, color: Color);
}

/// Initializes graphics
///
/// # Safety
///
/// must only be called once during initialization.
/// Requires logging and heap access.
pub unsafe fn init() {
    let fb: Framebuffer = take_boot_framebuffer()
        .expect("No framebuffer found")
        .into();

    unsafe {
        HARDWARE_FRAMEBUFFER_START = Some(fb.start);
    }

    framebuffer().lock_uninit().write(fb);
}

fn take_boot_framebuffer() -> Option<BootFrameBuffer> {
    let boot_info = boot_info();
    let fb = core::mem::replace(&mut boot_info.framebuffer, Optional::None);
    fb.into_option()
}
