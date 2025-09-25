//! Provides [OwnedSlice]

use core::{
    fmt::Debug,
    marker::PhantomData,
    ops::Range,
    ops::{Deref, DerefMut},
};

use alloc::{boxed::Box, vec::Vec};

/// A `&[T]` that also keeps ownership of the data
///
/// This can be used to return a subslice into a Box, including the box itself.
pub struct OwnedSlice<T, O> {
    owner: O,
    start: usize,
    end: usize,
    _phantom: PhantomData<T>,
}

impl<T, O> OwnedSlice<T, O> {
    /// Returns the underlying data and the range
    pub fn into_owned(self) -> (O, Range<usize>) {
        (self.owner, self.start..self.end)
    }
}

impl<T, O: AsRef<[T]>> OwnedSlice<T, O> {
    /// Creates a subset of the current [OwnedSlcie].
    ///
    /// This is similar to `&slice[range]` but transferes ownership to the new [OwnedSlice]
    pub fn subrange(self, range: Range<usize>) -> Self {
        assert!(self.len() >= range.len());

        let start = self.start + range.start;
        let end = start + range.len();

        Self {
            owner: self.owner,
            start,
            end,
            _phantom: PhantomData,
        }
    }
}

impl<T> From<Box<[T]>> for OwnedSlice<T, Box<[T]>> {
    fn from(value: Box<[T]>) -> Self {
        let end = value.len();
        Self {
            owner: value,
            start: 0,
            end,
            _phantom: PhantomData,
        }
    }
}

impl<T> From<Vec<T>> for OwnedSlice<T, Box<[T]>> {
    fn from(value: Vec<T>) -> Self {
        let boxed: Box<[T]> = value.into();
        boxed.into()
    }
}

impl<T, O: AsRef<[T]>> AsRef<[T]> for OwnedSlice<T, O> {
    fn as_ref(&self) -> &[T] {
        &self.owner.as_ref()[self.start..self.end]
    }
}

impl<T, O: AsMut<[T]>> AsMut<[T]> for OwnedSlice<T, O> {
    fn as_mut(&mut self) -> &mut [T] {
        &mut self.owner.as_mut()[self.start..self.end]
    }
}

impl<T, O: AsRef<[T]>> Deref for OwnedSlice<T, O> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T, O: AsMut<[T]> + AsRef<[T]>> DerefMut for OwnedSlice<T, O> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

impl<T: Debug, O: AsRef<[T]>> core::fmt::Debug for OwnedSlice<T, O> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OwnedSlice")
            .field_with("owner", |f| {
                f.write_fmt(format_args!("{:#p}", self.owner.as_ref()))
            })
            .field("start", &self.start)
            .field("end", &self.end)
            .field_with("entries", |f| {
                f.debug_list().entries(self.as_ref()).finish()
            })
            .finish()
    }
}

impl<T: PartialEq, O: AsRef<[T]>> PartialEq for OwnedSlice<T, O> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl<T: Eq, O: AsRef<[T]>> Eq for OwnedSlice<T, O> {}

impl<T, O: Clone> Clone for OwnedSlice<T, O> {
    fn clone(&self) -> Self {
        Self {
            owner: self.owner.clone(),
            start: self.start,
            end: self.end,
            _phantom: PhantomData,
        }
    }
}
