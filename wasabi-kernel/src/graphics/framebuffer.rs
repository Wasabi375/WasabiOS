//! utilites for framebuffer access

use bootloader_api::info::FrameBuffer;

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

/// clears the framebuffer to the given color
///
/// uses [clear_frame_buffer_rgb] internally
pub fn clear_frame_buffer(fb: &mut FrameBuffer, c: Color) {
    clear_frame_buffer_rgb(fb, c.r, c.g, c.b)
}

/// clears the framebuffer to the given rgb color
pub fn clear_frame_buffer_rgb(fb: &mut FrameBuffer, r: u8, g: u8, b: u8) {
    let info = fb.info();
    let buffer = fb.buffer_mut();

    for y in 0..info.height {
        for x in 0..info.width {
            let pos = info.bytes_per_pixel * info.stride * y + x * info.bytes_per_pixel;
            match info.pixel_format {
                bootloader_api::info::PixelFormat::Rgb => {
                    buffer[pos] = r;
                    buffer[pos + 1] = g;
                    buffer[pos + 2] = b;
                }
                bootloader_api::info::PixelFormat::Bgr => {
                    buffer[pos] = b;
                    buffer[pos + 1] = g;
                    buffer[pos + 2] = r;
                }
                bootloader_api::info::PixelFormat::U8 => {
                    let gray = ((r as u32 + g as u32 + b as u32) / 3) as u8;
                    buffer[pos] = gray;
                }
                _ => panic!("unknown pixel format"),
            }
        }
    }
}
