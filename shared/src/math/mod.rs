//! Math utilities

mod mod_group;

use core::{
    iter::Step,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign},
};

pub use mod_group::*;

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
    + Step
{
    /// The lowest representable value
    const MIN: Self;
    /// The largest representable value
    const MAX: Self;
    /// the zero value
    const ZERO: Self;
    /// the 1 value
    const ONE: Self;
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
    ($t:ident) => {
        impl Number for $t {
            const MIN: Self = Self::MIN;
            const MAX: Self = Self::MAX;
            const ZERO: Self = 0;
            const ONE: Self = 1;
        }

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
    ($t:ident) => {
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
