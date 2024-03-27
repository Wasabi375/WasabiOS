//! structures for the [mem](crate::mem) module that don't fit anywhere more specific

use core::ops::{Deref, DerefMut};
use shared::lockcell::LockCell;
use x86_64::{
    structures::paging::{mapper::UnmapError, Mapper, Page, PageSize, PageTableFlags, Size4KiB},
    VirtAddr,
};

use crate::map_page;

use super::{
    frame_allocator::WasabiFrameAllocator,
    page_table::{PageTableKernelFlags, KERNEL_PAGE_TABLE},
    MemError,
};

/// Marker trait for things that can be memory mapped.
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
    pub count: u64,
}

impl<S: PageSize> Mappable for Pages<S> {}

impl<S: PageSize> Pages<S> {
    /// the start addr of the fist page
    pub fn start_addr(&self) -> VirtAddr {
        self.first_page.start_address()
    }

    /// the total size in bytes of all consecutive pages
    pub fn size(&self) -> u64 {
        S::SIZE * self.count
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

// TODO this should be generic over page size, but WasabiFrameAllocator::get_for_kernel
//      only works if this is specified
impl Unmapped<GuardedPages<Size4KiB>> {
    /// allocates [PhysFrames] and maps `self` to the allocated frames.
    pub fn alloc_and_map(self) -> Result<Mapped<GuardedPages<Size4KiB>>, MemError> {
        let pages = self.0;

        let mut frame_allocator = WasabiFrameAllocator::<Size4KiB>::get_for_kernel().lock();

        for page in pages.iter() {
            let frame = frame_allocator.alloc().ok_or(MemError::OutOfMemory)?;
            unsafe {
                // page is unmapped
                // TODO map_page should optionally take the page table to avoid
                //      locking and unlocking in a loop
                map_page!(
                    page,
                    Size4KiB,
                    PageTableFlags::WRITABLE | PageTableFlags::PRESENT | PageTableFlags::NO_EXECUTE,
                    frame,
                    frame_allocator.as_mut()
                )?;
            }
        }

        if pages.head_guard.is_none() && pages.tail_guard.is_none() {
            return Ok(Mapped(pages));
        }

        unsafe {
            // Safety: frame used for guard pages
            let guard_frame = frame_allocator.guard_frame().ok_or(MemError::OutOfMemory)?;

            if let Some(head_guard) = pages.head_guard {
                // head_guard is unmapped and we are mapping to the guard_frame
                map_page!(
                    head_guard.0,
                    Size4KiB,
                    PageTableFlags::GUARD,
                    guard_frame,
                    frame_allocator.as_mut()
                )?;
            }
            if let Some(tail_guard) = pages.tail_guard {
                // tail_guard is unmapped and we are mapping to the guard_frame
                map_page!(
                    tail_guard.0,
                    Size4KiB,
                    PageTableFlags::GUARD,
                    guard_frame,
                    frame_allocator.as_mut()
                )?;
            }
        }

        Ok(Mapped(pages))
    }
}

impl Mapped<GuardedPages<Size4KiB>> {
    /// unmaps `self` and deallocates the corresponding [PhysFrames]
    ///
    /// Safety:
    /// The caller must ensure that the pages are no longer used
    pub unsafe fn unmap_and_free(self) -> Result<Unmapped<GuardedPages<Size4KiB>>, UnmapError> {
        let pages = self.0;

        let mut frame_allocator = WasabiFrameAllocator::<Size4KiB>::get_for_kernel().lock();
        let mut page_table = KERNEL_PAGE_TABLE.lock();

        for page in pages.iter() {
            let (frame, flusher) = page_table.unmap(page)?;
            flusher.flush();
            unsafe {
                // Safety: see fn requirements
                frame_allocator.free(frame);
            }
        }

        if let Some(guard) = pages.head_guard {
            page_table.unmap(guard.0)?.1.flush();
        }
        if let Some(guard) = pages.tail_guard {
            page_table.unmap(guard.0)?.1.flush();
        }

        Ok(Unmapped(pages))
    }
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
    count: u64,
    // TODO this is technically a bug because count might not fit into a i64
    index: i64,
}

impl<S: PageSize> Iterator for PagesIter<S> {
    type Item = Page<S>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.count as i64 {
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
