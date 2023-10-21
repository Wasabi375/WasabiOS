//! Module containing math helpers for the Kernel

/// Multiplication by a percentage represented in a discrete value.
///
/// # Examples
///
/// multiplying to rgb8 Colors: `white * white` should still result in pure
/// white, therefor `256*256 = 256`.
/// The multiplication represents converting both values to floating point (0-1),
/// multiplying and reverting back to integer. However it is faster and easier
/// to implement this as `(a * b) / 256` in a 16-bit register
pub trait DiscreteMul<Rhs = Self> {
    /// the result type of the multiplication
    type Output;

    /// calculates the discrete product
    fn mul_discrete(self, rhs: Rhs) -> Self::Output;
}

impl DiscreteMul for u8 {
    type Output = Self;

    fn mul_discrete(self, rhs: Self) -> Self::Output {
        (((self as u16) * (rhs as u16)) / 256) as u8
    }
}
