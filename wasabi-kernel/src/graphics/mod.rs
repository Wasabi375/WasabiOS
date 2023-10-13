//! Module containing all graphics related code for the kernel

pub mod framebuffer;
pub mod kernel_font;

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
