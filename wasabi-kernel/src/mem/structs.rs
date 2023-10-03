//! structures for the [mem](crate::mem) module that don't fit anywhere more specific

use core::ops::{Deref, DerefMut};
use x86_64::{
    structures::paging::{Page, PageSize, Size4KiB},
    VirtAddr,
};

/// Marker trati for things that can be memory mapped.
pub trait Mappable {}

/// A memory mapped [T]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Mapped<T: Mappable>(pub T);

impl<T: Mappable> From<T> for Mapped<T> {
    fn from(value: T) -> Mapped<T> {
        Mapped(value)
    }
}

/// A [T] that is not yet mapped.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Unmapped<T: Mappable>(pub T);

impl<T: Mappable> From<T> for Unmapped<T> {
    fn from(value: T) -> Unmapped<T> {
        Unmapped(value)
    }
}

impl<S: PageSize> Mappable for Page<S> {}

/// a number of consecutive pages in virtual memory.
///
/// [Pages] only provides raw access to the virtual memory and does not imply
/// mapping either way. The page may or may not be mapped at any point in time.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Pages<S: PageSize> {
    /// the first page
    pub first_page: Page<S>,
    /// the number of consecutive pages in virtual memory
    pub count: usize,
}

impl<S: PageSize> Mappable for Pages<S> {}

impl<S: PageSize> Pages<S> {
    /// the start addr of the fist page
    pub fn start_addr(&self) -> VirtAddr {
        self.first_page.start_address()
    }

    /// the total size in bytes of all consecutive pages
    pub fn size(&self) -> u64 {
        S::SIZE * self.count as u64
    }

    /// the end addr (inclusive) of the last page
    pub fn end_addr(&self) -> VirtAddr {
        (self.first_page + self.count as u64 - 1).start_address() + (S::SIZE - 1)
    }

    /// iterates over all consecutive pages
    pub fn iter(&self) -> PagesIter<S> {
        PagesIter {
            first_page: self.first_page,
            count: self.count,
            index: 0,
        }
    }
}

/// a number of consecutive pages in virtual memory with 2 additional optional guard pages
/// guard pages are allways 4KiB in size.
///
/// Guard pages are never mapped
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct GuardedPages<S: PageSize> {
    /// optional guard page before [GuardedPages::pages]
    pub head_guard: Option<Unmapped<Page<Size4KiB>>>,
    /// optional guard page after [GuardedPages::pages]
    pub tail_guard: Option<Unmapped<Page<Size4KiB>>>,

    /// the pages
    pub pages: Pages<S>,
}

impl<S: PageSize> Mappable for GuardedPages<S> {}

impl<S: PageSize> Deref for GuardedPages<S> {
    type Target = Pages<S>;

    fn deref(&self) -> &Self::Target {
        &self.pages
    }
}

impl<S: PageSize> DerefMut for GuardedPages<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pages
    }
}

/// an double ended iterator over consecutive pages
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagesIter<S: PageSize> {
    first_page: Page<S>,
    count: usize,
    index: isize,
}

impl<S: PageSize> Iterator for PagesIter<S> {
    type Item = Page<S>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.count as isize {
            None
        } else {
            let res = Some(self.first_page + self.index as u64);
            self.index += 1;
            res
        }
    }
}

impl<S: PageSize> DoubleEndedIterator for PagesIter<S> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.index < 0 {
            None
        } else {
            let res = Some(self.first_page + self.index as u64);
            self.index -= 1;
            res
        }
    }
}

impl<S: PageSize> PartialOrd for PagesIter<S> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        if self.first_page != other.first_page {
            None
        } else {
            self.index.partial_cmp(&other.index)
        }
    }
}
