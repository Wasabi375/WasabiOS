//! A mathmatical group using multiplication modulus n.
//!
//! See https://en.wikipedia.org/wiki/Multiplicative_group_of_integers_modulo_n

use core::{
    fmt::{LowerHex, UpperHex},
    ops::{Add, AddAssign, Rem, Sub},
};

use super::{CheckedAdd, CheckedSub, UnsingedNumber};

/// A number that automatically wrapps at [Self::modulo].
// TODO implement Sub, Mul and Div
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WrappingValue<T: UnsingedNumber> {
    value: T,
    modulo: T,
}

impl<T: UnsingedNumber> WrappingValue<T> {
    /// Creates a new [WrappingValue] with the value 0
    pub fn zero(modulo: T) -> Self {
        Self {
            value: T::ZERO,
            modulo,
        }
    }

    /// Creates a new [WrappingValue]
    pub fn new(value: T, modulo: T) -> Self {
        Self { value, modulo }
    }

    /// The value
    pub fn value(self) -> T {
        self.value
    }

    /// The modulo used for all calculations
    ///
    /// This is also m
    pub fn modulo(self) -> T {
        self.modulo
    }
}

impl<T: UnsingedNumber> WrappingValue<T>
where
    T: Sub<Output = T>,
{
    /// The highest valid value using this [Self::modulo].
    pub fn max(self) -> T {
        self.modulo - T::ONE
    }
}

impl<T> PartialOrd for WrappingValue<T>
where
    T: PartialOrd + UnsingedNumber,
{
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        self.value.partial_cmp(&other.value)
    }
}

impl<T> Ord for WrappingValue<T>
where
    T: Ord + UnsingedNumber,
{
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}

impl<T> Add<T> for WrappingValue<T>
where
    T: UnsingedNumber,
    T: CheckedAdd<T, Output = T>,
    T: CheckedSub<T, Output = T>,
    T: Rem<T, Output = T>,
{
    type Output = Self;

    fn add(self, rhs: T) -> Self::Output {
        let value = match self.value.checked_add(rhs) {
            Some(value) => value % self.modulo,
            None => rhs
                .checked_sub(
                    self.modulo
                        .checked_sub(self.value)
                        .expect("value is larger than modulo"),
                )
                .expect("rhs should be larger than `value - modulo`")
                .rem(self.modulo),
        };
        Self {
            value,
            modulo: self.modulo,
        }
    }
}

impl<T> AddAssign<T> for WrappingValue<T>
where
    T: UnsingedNumber,
    WrappingValue<T>: Add<T, Output = Self>,
{
    fn add_assign(&mut self, rhs: T) {
        *self = self.add(rhs);
    }
}

impl<T> core::fmt::Debug for WrappingValue<T>
where
    T: UnsingedNumber + core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("({:?} % {:?})", self.value, self.modulo))
    }
}

impl<T> LowerHex for WrappingValue<T>
where
    T: UnsingedNumber + LowerHex,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("({:#x} % {:#x})", self.value, self.modulo))
    }
}

impl<T> UpperHex for WrappingValue<T>
where
    T: UnsingedNumber + UpperHex,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("({:#X} % {:#X})", self.value, self.modulo))
    }
}

#[cfg(test)]
mod test {
    use super::WrappingValue;

    #[test]
    fn test_wrapping() {
        let mut wrapping = WrappingValue::zero(5u8);

        wrapping += 4;
        assert_eq!(wrapping.value(), 4);

        wrapping += 1;
        assert_eq!(wrapping.value(), 0);

        wrapping += 5;
        assert_eq!(wrapping.value(), 0);

        wrapping += 7;
        assert_eq!(wrapping.value(), 2);
    }

    #[test]
    fn test_wrapping_overflow() {
        let mut wrapping = WrappingValue::zero(5u8);

        wrapping += 4;
        assert_eq!(wrapping.value(), 4);

        wrapping += 255;
        assert_eq!(wrapping.value(), 4);

        wrapping += 254;
        assert_eq!(wrapping.value(), 3);
    }
}
