use core::num::TryFromIntError;

use crate::math::DiscreteMul;

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

impl Color {
    /// Creates a new [Color]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// linear interpolate between a and b
    ///
    /// Fails with [TryFromIntError] if the result for any chanel, does not fit
    /// within a u8
    pub fn lerp(a: Color, b: Color, t: f32) -> Result<Color, TryFromIntError> {
        Ok(Self {
            r: ((a.r as f32 * t + b.r as f32 * (1.0 - t)) as i32).try_into()?,
            g: ((a.g as f32 * t + b.g as f32 * (1.0 - t)) as i32).try_into()?,
            b: ((a.b as f32 * t + b.b as f32 * (1.0 - t)) as i32).try_into()?,
        })
    }

    /// linear interpolate between a and b using [MulDiscrete]
    pub fn lerp_discrete(a: Color, b: Color, t: u8) -> Self {
        Self {
            r: a.r.mul_discrete(t) + b.r.mul_discrete(255 - t),
            g: a.g.mul_discrete(t) + b.g.mul_discrete(255 - t),
            b: a.b.mul_discrete(t) + b.b.mul_discrete(255 - t),
        }
    }
}

impl DiscreteMul<u8> for Color {
    type Output = Self;

    fn mul_discrete(self, rhs: u8) -> Self::Output {
        Self {
            r: self.r.mul_discrete(rhs),
            g: self.g.mul_discrete(rhs),
            b: self.b.mul_discrete(rhs),
        }
    }
}
