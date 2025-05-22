//! Math utilities

mod mod_group;

use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign};

pub use mod_group::*;
use simple_endian::LittleEndian;

/// A utility trait for all number types
pub trait Number:
    Copy
    + Add<Self, Output = Self>
    + Sub<Self, Output = Self>
    + Mul<Self, Output = Self>
    + Div<Self, Output = Self>
    + AddAssign
    + SubAssign
    + MulAssign
    + DivAssign
    + PartialEq
    + Eq
    + PartialOrd
    + Ord
{
    /// The lowest representable value
    const MIN: Self;
    /// The largest representable value
    const MAX: Self;
    /// the value zero
    const ZERO: Self;
    /// the value one
    const ONE: Self;
}

/// A helper trait to generate constants for [Number]s.
#[const_trait]
pub trait NumberConstants {
    /// Creates a [Number] of `value`.
    ///
    /// This can be used instead of literals
    fn constant(value: usize) -> Self;
}

impl<T> const NumberConstants for T
where
    T: Number + ~const core::ops::Add,
{
    #[allow(clippy::assign_op_pattern)]
    fn constant(value: usize) -> Self {
        let mut res = Self::ZERO;
        let mut count = 0;
        while count < value {
            res = res + Self::ONE;
            count += 1;
        }
        res
    }
}

/// Types that implement this can be converted into a u64.
pub trait IntoU64 {
    /// Converts `self` into u64
    fn into(self) -> u64;
}

/// Types that implement this can be converted into a i64.
pub trait IntoI64 {
    /// Converts `self` into i64
    fn into(self) -> i64;
}

/// Types that implement this can be converted into a usize.
pub trait IntoUSize {
    /// Converts `self` into i64
    fn into(self) -> usize;
}

/// Types that implement this can be converted into a usize.
pub trait IntoISize {
    /// Converts `self` into isize
    fn into(self) -> isize;
}

macro_rules! impl_into_ui64 {
    ($typ:ident) => {
        impl IntoU64 for $typ {
            fn into(self) -> u64 {
                self as u64
            }
        }
        impl IntoI64 for $typ {
            fn into(self) -> i64 {
                self as i64
            }
        }

        impl IntoUSize for $typ {
            fn into(self) -> usize {
                self as usize
            }
        }
        impl IntoISize for $typ {
            fn into(self) -> isize {
                self as isize
            }
        }
    };
}

impl_into_ui64!(u8);
impl_into_ui64!(u16);
impl_into_ui64!(u32);
impl_into_ui64!(u64);
impl_into_ui64!(usize);
impl_into_ui64!(u128);
impl_into_ui64!(i8);
impl_into_ui64!(i16);
impl_into_ui64!(i32);
impl_into_ui64!(i64);
impl_into_ui64!(isize);
impl_into_ui64!(i128);

/// A utility trait for all unsinged number types
pub trait UnsingedNumber: Number {}

/// Wrapping additon
pub trait WrappingAdd<T>: Sized {
    /// The result of the wrapping operation
    type Output;

    /// Performs the add opertion or returns None on an overflow
    fn wrapping_add(self, rhs: T) -> Self::Output;
}

/// Wrapping subtraction
pub trait WrappingSub<T>: Sized {
    /// The result of the wrapping operation
    type Output;

    /// Performs the sub opertion or returns None on an overflow
    fn wrapping_sub(self, rhs: T) -> Self::Output;
}

/// Wrapping division
pub trait WrappingDiv<T>: Sized {
    /// The result of the wrapping operation
    type Output;

    /// Performs the div opertion or returns None on an overflow
    fn wrapping_div(self, rhs: T) -> Self::Output;
}

/// Wrapping multiplication
pub trait WrappingMul<T>: Sized {
    /// The result of the wrapping operation
    type Output;

    /// Performs the mul opertion or returns None on an overflow
    fn wrapping_mul(self, rhs: T) -> Self::Output;
}

/// Checked additon
pub trait CheckedAdd<T>: Sized {
    /// The result of the checked operation
    type Output;

    /// Performs the add opertion or returns None on an overflow
    fn checked_add(self, rhs: T) -> Option<Self::Output>;
}

/// Checked subtraction
pub trait CheckedSub<T>: Sized {
    /// The result of the checked operation
    type Output;

    /// Performs the sub opertion or returns None on an overflow
    fn checked_sub(self, rhs: T) -> Option<Self::Output>;
}

/// Checked division
pub trait CheckedDiv<T>: Sized {
    /// The result of the checked operation
    type Output;

    /// Performs the div opertion or returns None on an overflow
    fn checked_div(self, rhs: T) -> Option<Self::Output>;
}

/// Checked multiplication
pub trait CheckedMul<T>: Sized {
    /// The result of the checked operation
    type Output;

    /// Performs the mul opertion or returns None on an overflow
    fn checked_mul(self, rhs: T) -> Option<Self::Output>;
}

macro_rules! impl_number {
    ($t:ty, $min:expr, $max:expr, $zero:expr, $one:expr) => {
        impl Number for $t {
            const MIN: Self = $min;
            const MAX: Self = $max;
            const ZERO: Self = $zero;
            const ONE: Self = $one;
        }
    };
    ($t:ty) => {
        impl_number!($t, Self::MIN, Self::MAX, 0, 1);
    };
}

macro_rules! impl_number_ops {
    ($t:ty) => {
        impl WrappingAdd<$t> for $t {
            type Output = $t;

            fn wrapping_add(self, rhs: Self) -> Self::Output {
                self.wrapping_add(rhs)
            }
        }
        impl WrappingSub<$t> for $t {
            type Output = $t;

            fn wrapping_sub(self, rhs: Self) -> Self::Output {
                self.wrapping_sub(rhs)
            }
        }
        impl WrappingDiv<$t> for $t {
            type Output = $t;

            fn wrapping_div(self, rhs: Self) -> Self::Output {
                self.wrapping_div(rhs)
            }
        }
        impl WrappingMul<$t> for $t {
            type Output = $t;

            fn wrapping_mul(self, rhs: Self) -> Self::Output {
                self.wrapping_mul(rhs)
            }
        }

        impl CheckedAdd<$t> for $t {
            type Output = $t;

            fn checked_add(self, rhs: Self) -> Option<Self::Output> {
                self.checked_add(rhs)
            }
        }
        impl CheckedSub<$t> for $t {
            type Output = $t;

            fn checked_sub(self, rhs: Self) -> Option<Self::Output> {
                self.checked_sub(rhs)
            }
        }
        impl CheckedDiv<$t> for $t {
            type Output = $t;

            fn checked_div(self, rhs: Self) -> Option<Self::Output> {
                self.checked_div(rhs)
            }
        }
        impl CheckedMul<$t> for $t {
            type Output = $t;

            fn checked_mul(self, rhs: Self) -> Option<Self::Output> {
                self.checked_mul(rhs)
            }
        }
    };
}

macro_rules! impl_unsinged {
    ($t:ty, $min:expr, $max:expr, $zero:expr, $one:expr) => {
        impl_number!($t, $min, $max, $zero, $one);
        impl UnsingedNumber for $t {}
    };
    ($t:ty) => {
        impl_number!($t);
        impl UnsingedNumber for $t {}
    };
}

impl_unsinged!(u8);
impl_unsinged!(u16);
impl_unsinged!(u32);
impl_unsinged!(u64);
impl_unsinged!(u128);
impl_unsinged!(usize);

impl_number!(i8);
impl_number!(i16);
impl_number!(i32);
impl_number!(i64);
impl_number!(i128);
impl_number!(isize);

impl_number_ops!(u8);
impl_number_ops!(u16);
impl_number_ops!(u32);
impl_number_ops!(u64);
impl_number_ops!(u128);
impl_number_ops!(usize);

impl_number_ops!(i8);
impl_number_ops!(i16);
impl_number_ops!(i32);
impl_number_ops!(i64);
impl_number_ops!(i128);
impl_number_ops!(isize);

impl_unsinged!(
    LittleEndian<u64>,
    LittleEndian::from_bits(u64::MIN.to_le()),
    LittleEndian::from_bits(u64::MAX.to_le()),
    LittleEndian::from_bits(0u64.to_le()),
    LittleEndian::from_bits(1u64.to_le())
);

#[cfg(test)]
mod test {
    use super::NumberConstants;
    #[test]
    fn test_const_number_literals() {
        assert_eq!(0u8, u8::constant(0));
        assert_eq!(1u16, u16::constant(1));
        assert_eq!(5u8, u8::constant(5));
        assert_eq!(4321u64, u64::constant(4321));
    }
}
