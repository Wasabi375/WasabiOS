//! Functonality for a simple Kernel font
//!
//! This module does not include loading and working woth true-type-fonts, etc.
//! Instead it only contains functionality for loading and displaying a single
//! mono font (noto-sans-mono).

use super::{Canvas, Color, Point};
use noto_sans_mono_bitmap::{
    get_raster, get_raster_width, FontWeight, RasterHeight, RasterizedChar,
};

/// height of a bitmap character in pixel
const CHAR_RASTER_HEIGHT: RasterHeight = RasterHeight::Size16;
/// width of a bitmap character in pixel
const CHAR_RASTER_WIDTH: u32 = get_raster_width(FONT_WEIGHT, CHAR_RASTER_HEIGHT) as u32;
/// the character used when the requested character is not available in the font.
const BACKUP_CHAR: char = '�';
/// the font weight of the bitmap font
const FONT_WEIGHT: FontWeight = FontWeight::Regular;

/// A Bitfont for the kernel.
#[derive(Debug, Clone, Copy)]
pub struct BitFont {
    weight: FontWeight,
    height: RasterHeight,
    width: u32,
}

impl BitFont {
    /// gets the kernel bitmap font
    pub fn get() -> Self {
        BitFont {
            weight: FONT_WEIGHT,
            height: CHAR_RASTER_HEIGHT,
            width: CHAR_RASTER_WIDTH,
        }
    }

    /// returns the height of the font in pixel
    pub fn line_height(&self) -> u32 {
        self.height as _
    }

    /// the width of a character in pixel
    pub fn char_width(&self) -> u32 {
        self.width
    }

    /// Returns the raster of the given char or the raster of [`font_constants::BACKUP_CHAR`].
    pub fn get_char_raster(&self, c: char) -> RasterizedChar {
        fn get(slf: &BitFont, c: char) -> Option<RasterizedChar> {
            get_raster(c, slf.weight, slf.height)
        }
        get(self, c)
            .unwrap_or_else(|| get(self, BACKUP_CHAR).expect("Should get raster of backup char."))
    }

    /// Draws the char to the [Canvas]
    pub fn draw_char<C: Canvas>(
        &self,
        c: char,
        pos: Point,
        color: Color,
        bg: Color,
        canvas: &mut C,
    ) {
        for (y, row) in self.get_char_raster(c).raster().iter().enumerate() {
            for (x, byte) in row.iter().enumerate() {
                let color = Color::lerp_discrete(color, bg, *byte);
                canvas.set_pixel(pos.x + x as u32, pos.y + y as u32, color);
            }
        }
    }
}
