//! Iterator Utilities

use core::iter::{FusedIterator, Peekable, TrustedLen};

/// Extension functions for iterators
pub trait IterExt: Sized + Iterator {
    /// Creates an iterator that provides the position as well as a flags for the first and last
    /// item
    ///
    /// # Example
    /// ```
    /// # use shared::iter::{IterExt, PositionInfo};
    ///
    /// let a = [1, 2, 3];
    /// let mut iter = a.iter().cloned().with_positions();
    ///
    /// assert_eq!(Some(PositionInfo { index: 0, first: true, last: false, item: 1}), iter.next());
    /// assert_eq!(Some(PositionInfo { index: 1, first: false, last: false, item: 2}), iter.next());
    /// assert_eq!(Some(PositionInfo { index: 2, first: false, last: true, item: 3}), iter.next());
    ///
    /// assert_eq!(None, iter.next())
    /// ```
    fn with_positions(self) -> WithPositionIter<Self> {
        WithPositionIter {
            iter: self.peekable(),
            is_first: true,
            index: 0,
        }
    }

    /// Creates an iterator that provieds the item of the underlying iterator as well as a clone of
    /// the next item.
    ///
    /// # Example
    /// ```
    /// # use shared::iter::{IterExt};
    ///
    /// let a = [1, 2, 3];
    /// let mut iter = a.iter().cloned().peeked();
    ///
    /// assert_eq!(Some((1, Some(2))), iter.next());
    /// assert_eq!(Some((2, Some(3))), iter.next());
    /// assert_eq!(Some((3, None)), iter.next());
    ///
    /// assert_eq!(None, iter.next())
    /// ```
    fn peeked(self) -> Peeked<Self>
    where
        <Self as Iterator>::Item: Clone,
    {
        Peeked(self.peekable())
    }
}

impl<T, I> IterExt for T where T: Iterator<Item = I> {}

/// An iterator that provides position as well as flags for the first and last
/// item
///
/// see [IterExt::with_positions]
pub struct WithPositionIter<I: Iterator> {
    iter: Peekable<I>,
    is_first: bool,
    index: usize,
}

impl<I> Clone for WithPositionIter<I>
where
    I: Iterator + Clone,
    <I as Iterator>::Item: Clone,
{
    fn clone(&self) -> Self {
        Self {
            iter: Clone::clone(&self.iter),
            is_first: self.is_first,
            index: self.index,
        }
    }
}

/// The Item returned by [WithPositionIter]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionInfo<I> {
    /// the index of the item
    pub index: usize,
    /// true if the item is the first item in the iterator
    pub first: bool,
    /// true if the item is the last item in the iterator
    pub last: bool,
    /// the item returned by the underlying iterator
    pub item: I,
}

impl<I> Iterator for WithPositionIter<I>
where
    I: Iterator,
{
    type Item = PositionInfo<<I as Iterator>::Item>;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.iter.next()?;

        let first = self.is_first;
        self.is_first = false;

        let index = self.index;
        self.index += 1;

        let last = self.iter.peek().is_none();

        Some(PositionInfo {
            index,
            first,
            last,
            item,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

/// An iterator that returns the item of the underlying iterator and a clone of the peeked item.
///
/// The result of the iterator is similar to `(iter.next(), iter.peek().clone())`.
///
/// see [IterExt::peeked], [Peekable]
pub struct Peeked<I: Iterator>(Peekable<I>);

impl<I> Iterator for Peeked<I>
where
    I: Iterator,
    <I as Iterator>::Item: Clone + Copy,
{
    type Item = (<I as Iterator>::Item, Option<<I as Iterator>::Item>);

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.0.next();
        let next = self.0.peek();
        current.map(|current| (current, next.cloned()))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<I> Clone for Peeked<I>
where
    I: Clone + Iterator,
    <I as Iterator>::Item: Clone,
{
    fn clone(&self) -> Self {
        Peeked(self.0.clone())
    }
}

impl<I> ExactSizeIterator for Peeked<I>
where
    I: ExactSizeIterator,
    <I as Iterator>::Item: Copy + Clone,
{
    fn len(&self) -> usize {
        self.0.len()
    }
}

impl<I> FusedIterator for Peeked<I>
where
    I: FusedIterator + ExactSizeIterator,
    <I as Iterator>::Item: Copy + Clone,
{
}

unsafe impl<I> TrustedLen for Peeked<I>
where
    I: TrustedLen + ExactSizeIterator,
    <I as Iterator>::Item: Copy + Clone,
{
}
