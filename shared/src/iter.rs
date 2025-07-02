//! Iterator Utilities

use core::iter::Peekable;

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

impl<Iter, Item> Iterator for WithPositionIter<Iter>
where
    Iter: Iterator<Item = Item>,
{
    type Item = PositionInfo<Item>;

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
