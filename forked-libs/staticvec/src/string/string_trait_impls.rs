use core::borrow::{Borrow, BorrowMut};
use core::cmp::Ordering;
use core::convert::Infallible;
use core::fmt::{self, Arguments, Debug, Display, Formatter, Write};
use core::hash::{Hash, Hasher};
use core::ops::{
    Add, AddAssign, Deref, DerefMut, Index, IndexMut, Range, RangeFrom, RangeFull, RangeInclusive,
    RangeTo, RangeToInclusive,
};
use core::str::{self, FromStr};
use shared::math::Number;

use super::StaticString;
use crate::{CapacityError, StaticVec};

#[cfg(feature = "alloc")]
use alloc::string::String;

#[cfg(feature = "serde")]
use serde::{de::Deserializer, ser::Serializer, Deserialize, Serialize};

impl<const N: usize, L> Add<&str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = Self;

    #[inline(always)]
    fn add(mut self, other: &str) -> Self::Output {
        self.push_str_truncating(other);
        self
    }
}

impl<const N: usize, L> AddAssign<&str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn add_assign(&mut self, other: &str) {
        self.push_str_truncating(other);
    }
}

impl<const N: usize, L> AsMut<str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn as_mut(&mut self) -> &mut str {
        self.as_mut_str()
    }
}

impl<const N: usize, L> AsRef<str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl<const N: usize, L> AsRef<[u8]> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<const N: usize, L> Borrow<str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl<const N: usize, L> BorrowMut<str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn borrow_mut(&mut self) -> &mut str {
        self.as_mut_str()
    }
}

impl<const N: usize, L> Clone for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn clone(&self) -> Self {
        Self {
            vec: self.vec.clone(),
        }
    }

    #[inline(always)]
    fn clone_from(&mut self, other: &Self) {
        // Calling `clone_from` directly through `self.vec` is not accepted by the compiler currently
        // for some reason in the context of this impl being `const`.
        unsafe {
            self.vec
                .as_mut_ptr()
                .copy_from_nonoverlapping(other.vec.as_ptr(), other.vec.length.try_into().unwrap());
            self.vec.set_len(other.vec.length);
        }
    }
}

impl<const N: usize, L> Debug for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.debug_struct("StaticString")
            .field("array", &self.as_str())
            .field("size", &self.len().try_into().unwrap())
            .finish()
    }
}

impl<const N: usize, L> Default for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, L> Deref for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Target = str;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl<const N: usize, L> DerefMut for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_str()
    }
}

impl<const N: usize, L> Display for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl<const N: usize, L> Eq for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
}

impl<const N: usize, L> Extend<char> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend<I: IntoIterator<Item = char>>(&mut self, iterable: I) {
        self.push_str_truncating(Self::from_chars(iterable))
    }
}

impl<'a, const N: usize, L> Extend<&'a char> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend<I: IntoIterator<Item = &'a char>>(&mut self, iter: I) {
        self.extend(iter.into_iter().copied());
    }
}

impl<'a, const N: usize, L> Extend<&'a str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend<I: IntoIterator<Item = &'a str>>(&mut self, iterable: I) {
        self.push_str_truncating(Self::from_iterator(iterable))
    }
}

impl<'a, const N: usize, L> TryFrom<&'a str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Error = CapacityError<N>;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        Self::try_from_str(value)
    }
}

impl<const N1: usize, const N2: usize, L> From<StaticVec<u8, N1, L>> for StaticString<N2, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    default fn from(vec: StaticVec<u8, N1, L>) -> Self {
        Self::from_utf8(vec.as_slice()).expect("Invalid UTF-8!")
    }
}

impl<const N: usize, L> From<StaticVec<u8, N, L>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn from(vec: StaticVec<u8, N, L>) -> Self {
        unsafe {
            Self::from_str_unchecked(core::str::from_utf8(vec.as_slice()).expect("Invalid UTF-8!"))
        }
    }
}

#[cfg(feature = "alloc")]
#[doc(cfg(feature = "alloc"))]
impl<const N: usize, L> From<String> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn from(string: String) -> Self {
        Self {
            vec: StaticVec::from_iter(string.into_bytes().iter()),
        }
    }
}

impl<const N: usize, L> FromIterator<char> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn from_iter<I: IntoIterator<Item = char>>(iter: I) -> Self {
        Self::from_chars(iter)
    }
}

impl<'a, const N: usize, L> FromIterator<&'a char> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn from_iter<I: IntoIterator<Item = &'a char>>(iter: I) -> Self {
        Self::from_chars(iter.into_iter().copied())
    }
}

impl<'a, const N: usize, L> FromIterator<&'a str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Self {
        Self::from_iterator(iter)
    }
}

impl<'a, const N: usize, L> FromStr for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Err = Infallible;

    #[inline(always)]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::from_str(s))
    }
}

impl<const N: usize, L> Hash for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.as_str().hash(hasher);
    }
}

impl<const N: usize, L> Index<Range<usize>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = str;

    #[inline(always)]
    fn index(&self, index: Range<usize>) -> &Self::Output {
        self.as_str().index(index)
    }
}

impl<const N: usize, L> IndexMut<Range<usize>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn index_mut(&mut self, index: Range<usize>) -> &mut str {
        self.as_mut_str().index_mut(index)
    }
}

impl<const N: usize, L> Index<RangeFrom<L>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = str;

    #[inline(always)]
    fn index(&self, index: RangeFrom<L>) -> &Self::Output {
        self.as_str().index(RangeFrom {
            start: index.start.try_into().unwrap(),
        })
    }
}

impl<const N: usize, L> IndexMut<RangeFrom<L>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn index_mut(&mut self, index: RangeFrom<L>) -> &mut str {
        self.as_mut_str().index_mut(RangeFrom {
            start: index.start.try_into().unwrap(),
        })
    }
}

impl<const N: usize> Index<RangeFull> for StaticString<N, usize> {
    type Output = str;

    #[inline(always)]
    fn index(&self, _index: RangeFull) -> &Self::Output {
        self.as_str()
    }
}

impl<const N: usize> IndexMut<RangeFull> for StaticString<N, usize> {
    #[inline(always)]
    fn index_mut(&mut self, _index: RangeFull) -> &mut str {
        self.as_mut_str()
    }
}

impl<const N: usize, L> Index<RangeInclusive<L>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = str;

    #[inline(always)]
    fn index(&self, index: RangeInclusive<L>) -> &Self::Output {
        self.as_str().index(RangeInclusive::new(
            (*index.start()).try_into().unwrap(),
            (*index.end()).try_into().unwrap(),
        ))
    }
}

impl<const N: usize, L> IndexMut<RangeInclusive<L>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn index_mut(&mut self, index: RangeInclusive<L>) -> &mut str {
        self.as_mut_str().index_mut(RangeInclusive::new(
            (*index.start()).try_into().unwrap(),
            (*index.end()).try_into().unwrap(),
        ))
    }
}

impl<const N: usize, L> Index<RangeTo<L>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = str;

    #[inline(always)]
    fn index(&self, index: RangeTo<L>) -> &Self::Output {
        self.as_str().index(RangeTo {
            end: index.end.try_into().unwrap(),
        })
    }
}

impl<const N: usize, L> IndexMut<RangeTo<L>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn index_mut(&mut self, index: RangeTo<L>) -> &mut str {
        self.as_mut_str().index_mut(RangeTo {
            end: index.end.try_into().unwrap(),
        })
    }
}

impl<const N: usize, L> Index<RangeToInclusive<L>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = str;

    #[inline(always)]
    fn index(&self, index: RangeToInclusive<L>) -> &Self::Output {
        self.as_str().index(RangeToInclusive {
            end: index.end.try_into().unwrap(),
        })
    }
}

impl<const N: usize, L> IndexMut<RangeToInclusive<L>> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn index_mut(&mut self, index: RangeToInclusive<L>) -> &mut str {
        self.as_mut_str().index_mut(RangeToInclusive {
            end: index.end.try_into().unwrap(),
        })
    }
}

impl<const N: usize, L> Ord for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl<const N: usize, L> PartialEq for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        self.as_str().eq(other.as_str())
    }
}

impl<const N: usize, L> PartialEq<str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn eq(&self, other: &str) -> bool {
        self.as_str().eq(other)
    }
}

impl<const N: usize, L> PartialEq<&str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn eq(&self, other: &&str) -> bool {
        self.as_str().eq(*other)
    }
}

#[cfg(feature = "alloc")]
#[doc(cfg(feature = "alloc"))]
impl<const N: usize, L> PartialEq<String> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn eq(&self, other: &String) -> bool {
        self.as_str().eq(other.as_str())
    }
}

impl<const N: usize, L> PartialOrd for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<const N: usize, L> PartialOrd<str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn partial_cmp(&self, other: &str) -> Option<Ordering> {
        Some(self.as_str().cmp(other))
    }
}

impl<const N: usize, L> PartialOrd<&str> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn partial_cmp(&self, other: &&str) -> Option<Ordering> {
        Some(self.as_str().cmp(*other))
    }
}

#[cfg(feature = "alloc")]
#[doc(cfg(feature = "alloc"))]
impl<const N: usize, L> PartialOrd<String> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn partial_cmp(&self, other: &String) -> Option<Ordering> {
        Some(self.as_str().cmp(other.as_str()))
    }
}

impl<const N: usize, L> Write for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.vec.write_str(s)
    }

    #[inline(always)]
    fn write_char(&mut self, c: char) -> fmt::Result {
        self.vec.write_char(c)
    }

    #[inline(always)]
    fn write_fmt(&mut self, args: Arguments<'_>) -> fmt::Result {
        self.vec.write_fmt(args)
    }
}

#[cfg(feature = "serde")]
#[doc(cfg(feature = "serde"))]
impl<'de, const N: usize, L> Deserialize<'de> for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <&str>::deserialize(deserializer).map(Self::from_str)
    }
}

#[cfg(feature = "serde")]
#[doc(cfg(feature = "serde"))]
impl<const N: usize, L> Serialize for StaticString<N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Serialize::serialize(self.as_str(), serializer)
    }
}
