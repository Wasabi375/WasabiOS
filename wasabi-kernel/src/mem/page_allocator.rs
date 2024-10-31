//! Page allocator implementation

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::{
    mem::{
        page_table::RecursivePageTableExt,
        structs::{GuardedPages, Pages},
        MemError, Result,
    },
    prelude::UnwrapTicketLock,
};
use shared::rangeset::{Range, RangeSet};
use x86_64::{
    structures::paging::{
        page_table::FrameError::FrameNotPresent, Page, PageSize, PageTable, PageTableFlags,
        PageTableIndex, RecursivePageTable, Size1GiB, Size2MiB, Size4KiB,
    },
    VirtAddr,
};

#[cfg(feature = "mem-stats")]
use super::stats::PageAllocStats;

/// the kernel page allocator
// safety: not accessed before it is initialized in [init]
static KERNEL_PAGE_ALLOCATOR: UnwrapTicketLock<PageAllocator> =
    unsafe { UnwrapTicketLock::new_uninit() };

/// the largets valid virt addr
///
/// this is not a canonical virt addr, but instead the largest value
/// that a virt addr can point to.
const MAX_VIRT_ADDR: u64 = 0x0000_ffff_ffff_ffff;

/// the index of the first page that can be allocated by
/// the kernel [PageAllocator].
///
/// It might be possible to allocate pages before this, but the allocator
/// will not do so automatically.
pub const MIN_ALLOCATED_PAGE: u64 = 2;

/// initialize the kernel page allocator.
///
/// The `page_table` is used to figure out, which part of the virt memory space
/// was allready allocated by the bootloader, e.g. kerenel code, stack, etc
pub fn init(page_table: &mut RecursivePageTable) {
    /// helper function to recursively iterate over l4, l3, l2 and l1 page tables
    fn recurse_page_tables_remove_mapped(
        level: u8,
        page_table: &PageTable,
        level_indices: [PageTableIndex; 4],
        vaddrs: &mut RangeSet,
    ) {
        for (i, entry) in page_table.iter().enumerate() {
            if !entry.flags().contains(PageTableFlags::PRESENT) {
                continue;
            }

            let mut level_indices = level_indices.clone();
            level_indices.rotate_left(1);
            level_indices[3] = PageTableIndex::new(i as u16);

            if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                match level {
                    3 => {
                        let l4: u64 = level_indices[2].into();
                        let l3: u64 = level_indices[3].into();
                        let virt_addr = (l4 << 39) | (l3 << 30);
                        vaddrs.remove(Range {
                            start: virt_addr,
                            end: virt_addr + (Size1GiB::SIZE - 1),
                        });
                    }
                    2 => {
                        let l4: u64 = level_indices[1].into();
                        let l3: u64 = level_indices[2].into();
                        let l2: u64 = level_indices[3].into();
                        let virt_addr = (l4 << 39) | (l3 << 30) | (l2 << 21);
                        vaddrs.remove(Range {
                            start: virt_addr,
                            end: virt_addr + (Size2MiB::SIZE - 1),
                        });
                    }
                    1 => {
                        warn!("found huge frame in page table level {level}");
                        continue;
                    }
                    _ => continue,
                }
            }

            if level == 1 {
                match entry.frame() {
                    Ok(_frame) => {
                        let l4: u64 = level_indices[0].into();
                        let l3: u64 = level_indices[1].into();
                        let l2: u64 = level_indices[2].into();
                        let l1: u64 = level_indices[3].into();
                        let virt_addr = (l4 << 39) | (l3 << 30) | (l2 << 21) | (l1 << 12);
                        vaddrs.remove(Range {
                            start: virt_addr,
                            end: virt_addr + (Size4KiB::SIZE - 1),
                        });
                    }
                    Err(FrameNotPresent) => {}
                    Err(other) => {
                        error!("unexpected error constructing page allocator: {other:?}");
                        let l4: u64 = level_indices[0].into();
                        let l3: u64 = level_indices[1].into();
                        let l2: u64 = level_indices[2].into();
                        let l1: u64 = level_indices[3].into();
                        let virt_addr = (l4 << 39) | (l3 << 30) | (l2 << 21) | (l1 << 12);
                        vaddrs.remove(Range {
                            start: virt_addr,
                            end: virt_addr + (Size4KiB::SIZE - 1),
                        });
                    }
                }
            } else {
                let l4_index: u64 = level_indices[0].into();
                let l3_index: u64 = level_indices[1].into();
                let l2_index: u64 = level_indices[2].into();
                let l1_index: u64 = level_indices[3].into();
                let entry_table_vaddr =
                    (l4_index << 39) | (l3_index << 30) | (l2_index << 21) | (l1_index << 12);

                // safety: entry_table_vaddr is a valid pointer to the next level page table
                // in a recursive page table
                let entry_table: &PageTable = unsafe { &*(entry_table_vaddr as *const PageTable) };
                recurse_page_tables_remove_mapped(level - 1, &entry_table, level_indices, vaddrs);
            }
        }
    }

    let mut vaddrs = RangeSet::new();
    vaddrs.insert(Range {
        start: MIN_ALLOCATED_PAGE * Size4KiB::SIZE,
        end: MAX_VIRT_ADDR,
    });

    info!("PageAllocator init from page table");
    let recursive_index = page_table.recursive_index();
    recurse_page_tables_remove_mapped(
        4,
        page_table.level_4_table(),
        [recursive_index; 4],
        &mut vaddrs,
    );

    // remove page table range from vaddrs
    let zero_index = PageTableIndex::new(0);
    let max_index = PageTableIndex::new(511);
    let page_table_start = RecursivePageTable::vaddr_from_index(
        recursive_index,
        zero_index,
        zero_index,
        zero_index,
        0,
    );
    let page_table_end_inclusive =
        RecursivePageTable::vaddr_from_index(recursive_index, max_index, max_index, max_index, 0);

    vaddrs.remove(Range {
        start: page_table_start.as_u64(),
        end: page_table_end_inclusive.as_u64(),
    });

    let mut page_allocator = PageAllocator {
        vaddrs,
        #[cfg(feature = "mem-stats")]
        stats: PageAllocStats::default(),
    };
    reserve_pages(&mut page_allocator);
    KERNEL_PAGE_ALLOCATOR.lock_uninit().write(page_allocator);
}

fn reserve_pages(page_allocator: &mut PageAllocator) {
    crate::apic::ap_startup::reserve_pages(page_allocator);
}

/// A page allocator
pub struct PageAllocator {
    /// rangeset used by the allocator to keep track of used virt mem
    vaddrs: RangeSet<256>,

    #[cfg(feature = "mem-stats")]
    stats: PageAllocStats,
}

impl PageAllocator {
    /// get access to the kernel's [PageAllocator]
    pub fn get_kernel_allocator() -> &'static UnwrapTicketLock<Self> {
        &KERNEL_PAGE_ALLOCATOR
    }

    /// tries to allocate the given page in this allocator.
    ///
    /// The idea is that you already have a [VirtAddr] that the caller wants to
    /// access, basically asking, can you allocate this specific page.
    ///
    /// Unless there is a good reason to allocate at a specific addr
    /// [PageAllocator::allocate_page] should be prefered.
    /// ```no_run
    /// let page = Page<Size4Kib>::containing_address(VirtAddr::new(0xff1234));
    /// match allocator.try_allocate_page(page) {
    ///     Ok(_) => {}, // page allocated
    ///     Err(e) => { panic!("failed to alloc page {page:?}"); }
    /// }
    /// ```
    pub fn try_allocate_page<S: PageSize>(&mut self, page: Page<S>) -> Result<()> {
        if page.start_address().as_u64() == 0 {
            return Err(MemError::NullAddress);
        }

        if self.vaddrs.remove_if_exist(Range {
            start: page.start_address().as_u64(),
            end: page.start_address().as_u64() + (S::SIZE - 1),
        }) {
            Ok(())
        } else {
            Err(MemError::PageInUse)
        }
    }

    /// allocates a new page of virtual memory.
    pub fn allocate_page<S: PageSize>(&mut self) -> Result<Page<S>> {
        let size = S::SIZE;
        let align = S::SIZE;

        let start = self.vaddrs.allocate(size, align);
        start
            .map(|s| VirtAddr::new(s as u64))
            .map(|s| {
                Page::from_start_address(s)
                    .expect("RangeSet should provide properly aligned addresses")
            })
            .ok_or(MemError::OutOfPages)
            .inspect(|_| {
                #[cfg(feature = "mem-stats")]
                self.stats.register_alloc::<S>(1);
            })
    }

    /// allocates multiple consecutive pages of virtual memory.
    pub fn allocate_pages<S: PageSize>(&mut self, count: u64) -> Result<Pages<S>> {
        let size = S::SIZE * count;
        let align = S::SIZE;

        let start = self.vaddrs.allocate(size, align);
        start
            .map(|s| VirtAddr::new(s as u64))
            .map(|s| {
                Page::from_start_address(s)
                    .expect("RangeSet should provide properly aligned addresses")
            })
            .map(|p| Pages::<S> {
                first_page: p,
                count,
            })
            .ok_or(MemError::OutOfPages)
            .inspect(|_| {
                #[cfg(feature = "mem-stats")]
                self.stats.register_alloc::<S>(count);
            })
    }

    /// allocate multiple consecutive pages, with additional guard pages.
    ///
    /// Guard pages are always 4KiB pages. Otherwise there is no difference
    /// between them and [GuardedPage::pages]. This is just a convenient utility
    /// to allocate `count` consecutive pages of size `S` with one additional
    /// 4KiB page at the front and back.
    pub fn allocate_guarded_pages<S: PageSize>(
        &mut self,
        count: u64,
        head_guard: bool, // TODO convert bool args to enum
        tail_guard: bool,
    ) -> Result<GuardedPages<S>> {
        if !head_guard && !tail_guard {
            let pages = self.allocate_pages(count)?;
            return Ok(GuardedPages {
                head_guard: None,
                tail_guard: None,
                pages,
            });
        }

        let size = {
            let mut size = S::SIZE * count;
            if head_guard {
                size += Size4KiB::SIZE;
            }
            if tail_guard {
                size += Size4KiB::SIZE;
            }
            size
        };
        let align = if head_guard {
            match S::SIZE {
                Size4KiB::SIZE => S::SIZE,
                Size2MiB::SIZE | Size1GiB::SIZE => {
                    // S::SIZE is 4KiB aligned and greater than 4KiB therefor
                    // SIZE - 4KiB is 4KiB aligned for the gurade page and the
                    // first page after the guard page is SIZE aligned. tail
                    // guard is always 4KiB aligned, because SIZE is 4KiB
                    // aligned
                    S::SIZE - Size4KiB::SIZE
                }
                _ => unreachable!(),
            }
        } else {
            S::SIZE
        };

        let mut start = self
            .vaddrs
            .allocate(size, align)
            .map(|s| VirtAddr::new(s as u64))
            .ok_or(MemError::OutOfPages)?;

        let head_guard = if head_guard {
            let guard = Page::from_start_address(start).expect("head_gurad should be aligned");
            start += Size4KiB::SIZE;
            Some(guard)
        } else {
            None
        };

        let first_page = Page::from_start_address(start).expect("first_page should be aligned");
        start += S::SIZE * count as u64;
        let pages = Pages { first_page, count };

        let tail_guard = if tail_guard {
            Some(Page::from_start_address(start).expect("tail guard should be aligned"))
        } else {
            None
        };

        Ok(GuardedPages {
            head_guard,
            tail_guard,
            pages,
        })
    }

    /// allocates a new page of virtual memory with size 4KiB.
    pub fn allocate_page_4k(&mut self) -> Result<Page<Size4KiB>> {
        self.allocate_page()
    }

    /// allocates a new page of virtual memory with size 2MiB.
    pub fn allocate_page_2m(&mut self) -> Result<Page<Size2MiB>> {
        self.allocate_page()
    }

    /// allocates a new page of virtual memory with size 1GiB.
    pub fn allocate_page_1g(&mut self) -> Result<Page<Size1GiB>> {
        self.allocate_page()
    }

    /// frees a page
    ///
    /// # Safety:
    /// Call must guarantee that there are no pointers into the page.
    pub unsafe fn free_page<S: PageSize>(&mut self, page: Page<S>) {
        if self.vaddrs.len() as usize == self.vaddrs.capacity() {
            // TODO this warning also aplies to try_allocate_page
            warn!("trying to free page({page:?}) when range set is at max len. This can panic unexpectedly");
        }
        self.vaddrs.insert(Range {
            start: page.start_address().as_u64(),
            end: page.start_address().as_u64() + (S::SIZE - 1),
        });
        #[cfg(feature = "mem-stats")]
        self.stats.register_free::<S>(1);
    }

    /// frees multiple pages
    ///
    /// # Safety:
    /// Call must guarantee that there are no pointers into the pages.
    pub unsafe fn free_pages<S: PageSize>(&mut self, pages: Pages<S>) {
        for p in pages.iter() {
            unsafe {
                self.free_page(p);
            }
        }
        #[cfg(feature = "mem-stats")]
        self.stats.register_free::<S>(pages.count);
    }

    /// frees multiple pages
    ///
    /// # Safety:
    /// Call must guarantee that there are no pointers into the pages.
    pub unsafe fn free_guarded_pages<S: PageSize>(&mut self, pages: GuardedPages<S>) {
        unsafe {
            self.free_pages(pages.pages);
            if let Some(guard) = pages.head_guard {
                self.free_page(guard);
            }
            if let Some(guard) = pages.tail_guard {
                self.free_page(guard);
            }
        }
    }

    /// Access to [PageAllocStats]
    #[cfg(feature = "mem-stats")]
    pub fn stats(&self) -> &PageAllocStats {
        &self.stats
    }
}

#[cfg(feature = "test")]
mod test {
    use core::assert_matches::assert_matches;

    use shared::sync::lockcell::LockCell;
    use testing::{kernel_test, KernelTestError};
    use x86_64::structures::paging::{Page, PageTableIndex, RecursivePageTable, Size4KiB};

    use crate::mem::{
        page_table::{RecursivePageTableExt, KERNEL_PAGE_TABLE},
        MemError,
    };

    use super::PageAllocator;

    #[kernel_test]
    fn test_cannot_alloc_page_table_page() -> Result<(), KernelTestError> {
        let mut page_table = KERNEL_PAGE_TABLE.lock();
        let recursive_index = page_table.recursive_index();
        drop(page_table);

        let some_index = PageTableIndex::new(42);

        let l4_vaddr = RecursivePageTable::l4_table_vaddr(recursive_index);
        let l3_vaddr = RecursivePageTable::l3_table_vaddr(recursive_index, some_index);
        let l2_vaddr = RecursivePageTable::l2_table_vaddr(recursive_index, some_index, some_index);
        let l1_vaddr =
            RecursivePageTable::l1_table_vaddr(recursive_index, some_index, some_index, some_index);

        let mut alloc = PageAllocator::get_kernel_allocator().lock();

        assert_matches!(
            alloc.try_allocate_page(Page::<Size4KiB>::from_start_address(l4_vaddr).unwrap()),
            Err(MemError::PageInUse),
            "Allocating l4 page table page should not be possible"
        );
        assert_matches!(
            alloc.try_allocate_page(Page::<Size4KiB>::from_start_address(l3_vaddr).unwrap()),
            Err(MemError::PageInUse),
            "Allocating l3 page table page should not be possible"
        );
        assert_matches!(
            alloc.try_allocate_page(Page::<Size4KiB>::from_start_address(l2_vaddr).unwrap()),
            Err(MemError::PageInUse),
            "Allocating l2 page table page should not be possible"
        );
        assert_matches!(
            alloc.try_allocate_page(Page::<Size4KiB>::from_start_address(l1_vaddr).unwrap()),
            Err(MemError::PageInUse),
            "Allocating l1 page table page should not be possible"
        );

        Ok(())
    }
}
