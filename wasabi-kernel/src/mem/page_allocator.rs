#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use super::{MemError, Result};
use crate::mem::page_table::RecursivePageTableExt;
use lazy_static::lazy_static;
use shared::{
    lockcell::SpinLock,
    rangeset::{Range, RangeSet},
};
use x86_64::{
    structures::paging::{
        page_table::FrameError::FrameNotPresent, Page, PageSize, PageTable, PageTableFlags,
        PageTableIndex, RecursivePageTable, Size1GiB, Size2MiB, Size4KiB,
    },
    VirtAddr,
};

lazy_static! {
    static ref KERNEL_PAGE_ALLOCATOR: SpinLock<PageAllocator> = SpinLock::default();
}

const PAGE_ALLOCATOR_MAX_RANGE_COUNT: u32 = 256;
pub struct PageAllocator {
    vaddrs: RangeSet<{ PAGE_ALLOCATOR_MAX_RANGE_COUNT as usize }>,
}

pub const MAX_VIRT_ADDR: u64 = 0xffffffffffff;

impl Default for PageAllocator {
    fn default() -> Self {
        PageAllocator::new()
    }
}

impl PageAllocator {
    pub(crate) fn new() -> Self {
        let mut vaddrs = RangeSet::new();
        vaddrs.insert(Range {
            start: 2 * Size4KiB::SIZE,
            end: MAX_VIRT_ADDR,
        });
        Self { vaddrs }
    }

    pub fn get_kernel_allocator() -> &'static SpinLock<Self> {
        &KERNEL_PAGE_ALLOCATOR
    }

    pub(super) fn init_from_page_table(&mut self, page_table: &mut RecursivePageTable) {
        fn recurse_page_tables(
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

                    let entry_table: &PageTable =
                        unsafe { &*(entry_table_vaddr as *const PageTable) };
                    recurse_page_tables(level - 1, &entry_table, level_indices, vaddrs);
                }
            }
        }

        info!("PageAllocator init from page table");
        let recursive_index = page_table.recursive_index();
        recurse_page_tables(
            4,
            page_table.level_4_table(),
            [recursive_index; 4],
            &mut self.vaddrs,
        );
    }

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
    }

    pub fn allocate_pages<S: PageSize>(&mut self, count: usize) -> Result<Pages<S>> {
        let size = S::SIZE * count as u64;
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
    }

    pub fn allocate_page_4k(&mut self) -> Result<Page<Size4KiB>> {
        self.allocate_page()
    }

    pub fn allocate_page_2m(&mut self) -> Result<Page<Size2MiB>> {
        self.allocate_page()
    }

    pub fn allocate_page_1g(&mut self) -> Result<Page<Size1GiB>> {
        self.allocate_page()
    }

    pub fn free_page<S: PageSize>(&mut self, page: Page<S>) {
        if self.vaddrs.len() == PAGE_ALLOCATOR_MAX_RANGE_COUNT {
            warn!("trying to free page({page:?}) when range set is at max len. This can panic unexpectedly");
        }
        self.vaddrs.insert(Range {
            start: page.start_address().as_u64(),
            end: page.start_address().as_u64() + (S::SIZE - 1),
        });
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Pages<S: PageSize> {
    pub first_page: Page<S>,
    pub count: usize,
}

impl<S: PageSize> Pages<S> {
    pub fn start_addr(&self) -> VirtAddr {
        self.first_page.start_address()
    }

    pub fn size(&self) -> u64 {
        S::SIZE * self.count as u64
    }

    pub fn end_addr(&self) -> VirtAddr {
        (self.first_page + self.count as u64 - 1).start_address() + (S::SIZE - 1)
    }

    pub fn iter(&self) -> PagesIter<S> {
        PagesIter {
            first_page: self.first_page,
            count: self.count,
            index: 0,
        }
    }
}

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
