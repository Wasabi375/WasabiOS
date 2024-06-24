//! structures for the [mem](crate::mem) module that don't fit anywhere more specific

use core::assert_matches::assert_matches;
use core::ops::{Deref, DerefMut};
use log::trace;
use shared::sync::lockcell::LockCell;
use x86_64::structures::paging::PageTableFlags;
use x86_64::{
    structures::paging::{
        mapper::{UnmapError, UnmappedFrame},
        Mapper, Page, PageSize, Size4KiB,
    },
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

        trace!("unmap guarded pages");
        for page in pages.iter() {
            let (frame, _flags, flusher) = page_table.unmap(page)?;
            flusher.flush();
            unsafe {
                // Safety: see fn requirements
                frame_allocator.free(frame);
            }
        }

        if let Some(guard) = pages.head_guard {
            assert_matches!(
                page_table.clear(guard.0)?,
                UnmappedFrame::NotPresent { entry: _ }
            );
        }
        if let Some(guard) = pages.tail_guard {
            assert_matches!(
                page_table.clear(guard.0)?,
                UnmappedFrame::NotPresent { entry: _ }
            );
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

#[cfg(feature = "test")]
#[doc(hidden)]
mod test {
    use shared::sync::lockcell::LockCell;
    use testing::{
        kernel_test, t_assert, t_assert_eq, t_assert_matches, tfail, DebugErrResultExt,
        KernelTestError, TestUnwrapExt,
    };
    use x86_64::structures::paging::{
        mapper::{MappedFrame, TranslateResult},
        PageSize, PageTableFlags, Size4KiB, Translate,
    };

    use crate::mem::{page_allocator::PageAllocator, page_table::KERNEL_PAGE_TABLE, VirtAddrExt};

    use super::Unmapped;

    #[kernel_test]
    fn alloc_guarded_page() -> Result<(), KernelTestError> {
        let mut allocator = PageAllocator::get_kernel_allocator().lock();

        let pages = allocator
            .allocate_guarded_pages::<Size4KiB>(1, true, true)
            .tunwrap()?;

        t_assert_eq!(pages.size(), Size4KiB::SIZE);

        t_assert!(pages.head_guard.is_some());
        t_assert!(pages.tail_guard.is_some());

        Ok(())
    }

    #[kernel_test]
    fn map_unmap_guarded_page() -> Result<(), KernelTestError> {
        let mut allocator = PageAllocator::get_kernel_allocator().lock();

        let pages = allocator
            .allocate_guarded_pages::<Size4KiB>(1, true, true)
            .texpect("failed to allocate guarded page")?;

        let unmapped = Unmapped(pages);
        let mapped = unmapped
            .alloc_and_map()
            .texpect("failed to map guarded page")?;

        {
            // lock page table for asserts.
            // Don't hold onto it for unmapping, as that might dead lock
            // with clearing the mapping from the page table
            let page_table = KERNEL_PAGE_TABLE.lock();
            let addr_in_page = mapped.0.first_page.start_address() + 50;

            // assert that the mapping is valid
            match page_table.translate(addr_in_page) {
                TranslateResult::Mapped {
                    frame,
                    offset,
                    flags,
                } => {
                    t_assert_matches!(frame, MappedFrame::Size4KiB(_));
                    t_assert_eq!(offset, 50);
                    t_assert_eq!(
                        flags,
                        PageTableFlags::WRITABLE
                            | PageTableFlags::PRESENT
                            | PageTableFlags::NO_EXECUTE
                    );
                }
                TranslateResult::NotMapped => {
                    tfail!("we called map but page is not mapped in page table")
                }
                TranslateResult::InvalidFrameAddress(_) => {
                    tfail!("page mapped to invalid phys addr")
                }
            }
            // try writing and reading in mapped page
            unsafe {
                // Safety: addr is a valid addr in an mapped page, and
                // any ptr is alliged for u8
                let mut ptr = addr_in_page.as_volatile_mut::<u8>();
                ptr.write(12);
                t_assert_eq!(ptr.read(), 12);
            }
        }

        unsafe {
            // Safety: we don't access any memory in the page. We only
            // allocated it to test that allocation is possible
            mapped
                .unmap_and_free()
                .map_err_debug_display()
                .texpect("failed to unmap guarded page")?;
        }

        {
            let page_table = KERNEL_PAGE_TABLE.lock();
            let addr_in_page = mapped.0.first_page.start_address() + 50;

            t_assert_matches!(
                page_table.translate(addr_in_page),
                TranslateResult::NotMapped,
                "unmapped guarded page is still mapped in page table"
            );
        }

        Ok(())
    }
}
