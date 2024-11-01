//! Page table implementation for the kernel

use core::ops::{Deref, DerefMut};

#[allow(unused_imports)]
use log::{debug, error, info, log, trace, warn};

use super::frame_allocator::FrameAllocator;
use crate::{kernel_info::KernelInfo, prelude::UnwrapTicketLock};

use thiserror::Error;
use x86_64::{
    structures::paging::{
        mapper::{MapToError, MapperFlush, UnmapError},
        Mapper, Page, PageSize, PageTable as X86PageTable, PageTableFlags, PageTableIndex,
        PhysFrame, RecursivePageTable, Size1GiB, Size2MiB, Size4KiB,
    },
    PhysAddr, VirtAddr,
};

/// A Page Table
pub struct PageTable<T> {
    inner: T,
}

/// the kernel [RecursivePageTable]
// Safety: not used before initialized in [init]
static KERNEL_PAGE_TABLE: UnwrapTicketLock<PageTable<RecursivePageTable>> =
    unsafe { UnwrapTicketLock::new_uninit() };

impl PageTable<RecursivePageTable<'static>> {
    /// get access to the kernel's [PageTable]
    pub fn get_for_kernel() -> &'static UnwrapTicketLock<Self> {
        &KERNEL_PAGE_TABLE
    }
}

impl<T> PageTable<T> {
    /// Creates a new page table
    pub const fn new(inner: T) -> Self {
        PageTable { inner }
    }

    /// Returns the underlying [Mapper] implementation
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// map a page for the kernel
    ///
    /// See: [Mapper::map_to_with_table_flags]
    ///
    /// # Safety:
    ///
    /// The caller must ensure that by creating this page-frame mapping all
    /// of rusts safety guarantees are fullfilled
    pub unsafe fn map_kernel<S>(
        &mut self,
        page: Page<S>,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        frame_allocator: &mut FrameAllocator,
    ) -> Result<MapperFlush<S>, PageTableMapError>
    where
        S: PageSize,
        T: Mapper<S>,
        PageTableMapError: From<MapToError<S>>,
    {
        unsafe {
            self.inner
                .map_to_with_table_flags(
                    page,
                    frame,
                    flags,
                    PageTableFlags::KERNEL_TABLE_FLAGS,
                    frame_allocator,
                )
                .map_err(|e| e.into())
        }
    }

    /// Remoev the mapping for the page
    ///
    /// See: [Mapper::unmap]
    pub fn unmap<S>(
        &mut self,
        page: Page<S>,
    ) -> Result<(PhysFrame<S>, PageTableFlags, MapperFlush<S>), PageTableMapError>
    where
        S: PageSize,
        T: Mapper<S>,
    {
        self.inner.unmap(page).map_err(|e| e.into())
    }
}

impl<T> Deref for PageTable<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for PageTable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T> AsRef<T> for PageTable<T> {
    fn as_ref(&self) -> &T {
        &self.inner
    }
}

impl<T> AsMut<T> for PageTable<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

/// kernel internal page table flags
pub trait PageTableKernelFlags {
    /// PageTableFlag denoting a guard page
    const GUARD: PageTableFlags = PageTableFlags::BIT_9;

    /// A combination of all PageTableFlags marking a page as used
    ///
    /// Some flags like e.g. `WRITABLE` onyl have meaning in combination
    /// with other flags. Others like `PRESENT` or `GUARD` can be used on
    /// their own. A page with those flags can not be repurposed.
    const PRESENT_OR_USED: PageTableFlags = {
        let mut flags = PageTableFlags::PRESENT;
        flags = flags.union(PageTableFlags::GUARD);
        flags
    };

    /// The flags used for the page table pages
    const KERNEL_TABLE_FLAGS: PageTableFlags = {
        let mut flags = PageTableFlags::PRESENT;
        flags = flags.union(PageTableFlags::WRITABLE);
        flags
    };
}

impl PageTableKernelFlags for PageTableFlags {}

#[derive(Error, Debug, PartialEq, Eq, Clone)]
#[allow(missing_docs)]
pub enum PageTableMapError {
    #[error("Failed to alloc frame")]
    FrameAllocationFailed,
    #[error("Part of page is already mapped as hzge page")]
    ParentEntryHugePage,
    #[error("The page is already mapped to a frame {0:?}")]
    PageAllreadyMapped4k(PhysFrame<Size4KiB>),
    #[error("The page is already mapped to a frame {0:?}")]
    PageAllreadyMapped2m(PhysFrame<Size2MiB>),
    #[error("The page is already mapped to a frame {0:?}")]
    PageAllreadyMapped1g(PhysFrame<Size1GiB>),
    #[error("the phys addr is not valid page address {0:p}")]
    InvalidFrameAddress(PhysAddr),
    #[error("unmapped page cannot be unmapped again")]
    PageNotMapped,
}

impl From<UnmapError> for PageTableMapError {
    fn from(value: UnmapError) -> Self {
        match value {
            UnmapError::ParentEntryHugePage => PageTableMapError::ParentEntryHugePage,
            UnmapError::PageNotMapped => PageTableMapError::PageNotMapped,
            UnmapError::InvalidFrameAddress(paddr) => PageTableMapError::InvalidFrameAddress(paddr),
        }
    }
}

// TODO: cleanup replace from impls with #[from] attribute
impl From<MapToError<Size4KiB>> for PageTableMapError {
    fn from(value: MapToError<Size4KiB>) -> Self {
        match value {
            MapToError::FrameAllocationFailed => PageTableMapError::FrameAllocationFailed,
            MapToError::ParentEntryHugePage => PageTableMapError::ParentEntryHugePage,
            MapToError::PageAlreadyMapped(f) => PageTableMapError::PageAllreadyMapped4k(f),
        }
    }
}

impl From<MapToError<Size2MiB>> for PageTableMapError {
    fn from(value: MapToError<Size2MiB>) -> Self {
        match value {
            MapToError::FrameAllocationFailed => PageTableMapError::FrameAllocationFailed,
            MapToError::ParentEntryHugePage => PageTableMapError::ParentEntryHugePage,
            MapToError::PageAlreadyMapped(f) => PageTableMapError::PageAllreadyMapped2m(f),
        }
    }
}

impl From<MapToError<Size1GiB>> for PageTableMapError {
    fn from(value: MapToError<Size1GiB>) -> Self {
        match value {
            MapToError::FrameAllocationFailed => PageTableMapError::FrameAllocationFailed,
            MapToError::ParentEntryHugePage => PageTableMapError::ParentEntryHugePage,
            MapToError::PageAlreadyMapped(f) => PageTableMapError::PageAllreadyMapped1g(f),
        }
    }
}

/// initialize the kernel page table
pub fn init(page_table: RecursivePageTable<'static>) {
    trace!("store kernel page table in lock");
    KERNEL_PAGE_TABLE
        .lock_uninit()
        .write(PageTable::new(page_table));
}

/// extension trait for [RecursivePageTable]
pub trait RecursivePageTableExt {
    /// calculate the l4 table addr based on the given recursive [PageTableIndex].
    fn l4_table_vaddr(r: PageTableIndex) -> VirtAddr;

    /// calculate the l3 table addr based on the given recursive [PageTableIndex].
    fn l3_table_vaddr(r: PageTableIndex, l4_index: PageTableIndex) -> VirtAddr;

    /// calculate the l2 table addr based on the given recursive [PageTableIndex].
    fn l2_table_vaddr(
        r: PageTableIndex,
        l4_index: PageTableIndex,
        l3_index: PageTableIndex,
    ) -> VirtAddr;

    /// calculate the l1 table addr based on the given recursive [PageTableIndex].
    fn l1_table_vaddr(
        r: PageTableIndex,
        l4_index: PageTableIndex,
        l3_index: PageTableIndex,
        l2_index: PageTableIndex,
    ) -> VirtAddr;

    /// get the recursive [PageTableIndex] of this page table
    fn recursive_index(&mut self) -> PageTableIndex;

    /// Calculate a [VirtAddr] based on the page table indices.
    ///
    /// The max value for the offset is `0xfff` or `4095`.
    fn vaddr_from_index(
        l4: PageTableIndex,
        l3: PageTableIndex,
        l2: PageTableIndex,
        l1: PageTableIndex,
        offset: u64,
    ) -> VirtAddr;
}

/// get the recursive [PageTableIndex] of the page table, provided by the bootloader.
#[inline]
pub fn recursive_index() -> PageTableIndex {
    PageTableIndex::new(KernelInfo::get().recursive_index)
}

impl<'a> RecursivePageTableExt for RecursivePageTable<'a> {
    #[inline]
    #[allow(dead_code)]
    fn recursive_index(&mut self) -> PageTableIndex {
        let vaddr = VirtAddr::new(self.level_4_table() as *const X86PageTable as u64);
        Page::<Size4KiB>::containing_address(vaddr).p4_index()
    }

    #[inline]
    #[allow(dead_code)]
    fn l4_table_vaddr(r: PageTableIndex) -> VirtAddr {
        let r: u64 = r.into();
        let vaddr = (r << 39) | (r << 30) | (r << 21) | (r << 12);

        VirtAddr::new(vaddr)
    }

    #[inline]
    #[allow(dead_code)]
    fn l3_table_vaddr(r: PageTableIndex, l4_index: PageTableIndex) -> VirtAddr {
        let r: u64 = r.into();
        let l4: u64 = l4_index.into();

        let vaddr = (r << 39) | (r << 30) | (r << 21) | (l4 << 12);

        VirtAddr::new(vaddr)
    }

    #[inline]
    #[allow(dead_code)]
    fn l2_table_vaddr(
        r: PageTableIndex,
        l4_index: PageTableIndex,
        l3_index: PageTableIndex,
    ) -> VirtAddr {
        let r: u64 = r.into();
        let l4: u64 = l4_index.into();
        let l3: u64 = l3_index.into();

        let vaddr = (r << 39) | (r << 30) | (l4 << 21) | (l3 << 12);

        VirtAddr::new(vaddr)
    }

    #[inline]
    #[allow(dead_code)]
    fn l1_table_vaddr(
        r: PageTableIndex,
        l4_index: PageTableIndex,
        l3_index: PageTableIndex,
        l2_index: PageTableIndex,
    ) -> VirtAddr {
        let r: u64 = r.into();
        let l4: u64 = l4_index.into();
        let l3: u64 = l3_index.into();
        let l2: u64 = l2_index.into();

        let vaddr = (r << 39) | (l4 << 30) | (l3 << 21) | (l2 << 12);

        VirtAddr::new(vaddr)
    }

    #[inline]
    #[allow(dead_code)]
    fn vaddr_from_index(
        l4: PageTableIndex,
        l3: PageTableIndex,
        l2: PageTableIndex,
        l1: PageTableIndex,
        offset: u64,
    ) -> VirtAddr {
        let l4: u64 = l4.into();
        let l3: u64 = l3.into();
        let l2: u64 = l2.into();
        let l1: u64 = l1.into();

        let vaddr = (l4 << 39) | (l3 << 30) | (l2 << 21) | (l1 << 12) | offset & 0xfff;

        VirtAddr::new(vaddr)
    }
}

#[cfg(feature = "test")]
mod test {
    use crate::mem::{frame_allocator::FrameAllocator, page_allocator::PageAllocator};
    use shared::sync::lockcell::LockCell;
    use testing::{
        kernel_test, t_assert_eq, t_assert_matches, tfail, DebugErrResultExt, KernelTestError,
        TestUnwrapExt,
    };
    use x86_64::structures::paging::{
        mapper::{MappedFrame, Mapper, TranslateResult, UnmappedFrame},
        Translate,
    };

    use super::*;

    #[kernel_test]
    fn test_map_unmap_fake_frame_not_present() -> Result<(), KernelTestError> {
        let fake_frame = PhysFrame::from_start_address(PhysAddr::new(0))
            .map_err_debug_display()
            .tunwrap()?;
        let page: Page<Size4KiB> = PageAllocator::get_for_kernel()
            .lock()
            .allocate_page::<Size4KiB>()
            .tunwrap()?;
        let frame_alloc: &mut FrameAllocator = &mut FrameAllocator::get_for_kernel().lock();
        let mut page_table = PageTable::get_for_kernel().lock();

        unsafe { page_table.map_to(page, fake_frame, PageTableFlags::BIT_9, frame_alloc) }
            .map(|flusher| flusher.flush())
            .map_err_debug_display()
            .tunwrap()?;

        let freed_frame = page_table.clear(page).map_err_debug_display().tunwrap()?;

        let UnmappedFrame::NotPresent { entry } = freed_frame else {
            tfail!("Expected to unmap not present page");
        };

        t_assert_eq!(entry.addr(), fake_frame.start_address());
        t_assert_eq!(entry.flags(), PageTableFlags::BIT_9);

        t_assert_matches!(
            page_table.translate(page.start_address()),
            TranslateResult::NotMapped,
        );

        unsafe {
            // Safety: page no longer used
            PageAllocator::get_for_kernel().lock().free_page(page);
        }

        Ok(())
    }

    #[kernel_test]
    fn test_fake_frame_not_present_update_flags() -> Result<(), KernelTestError> {
        let fake_frame = PhysFrame::from_start_address(PhysAddr::new(0))
            .map_err_debug_display()
            .tunwrap()?;
        let page: Page<Size4KiB> = PageAllocator::get_for_kernel()
            .lock()
            .allocate_page::<Size4KiB>()
            .tunwrap()?;
        let frame_alloc: &mut FrameAllocator = &mut FrameAllocator::get_for_kernel().lock();
        let mut page_table = PageTable::get_for_kernel().lock();
        unsafe { page_table.map_to(page, fake_frame, PageTableFlags::BIT_9, frame_alloc) }
            .map(|flusher| flusher.flush())
            .map_err_debug_display()
            .tunwrap()?;

        unsafe { page_table.update_flags(page, PageTableFlags::BIT_10) }
            .map(|flusher| flusher.flush())
            .map_err_debug_display()
            .tunwrap()?;

        if let TranslateResult::Mapped {
            frame,
            offset,
            flags,
        } = page_table.translate(page.start_address())
        {
            t_assert_matches!(frame, MappedFrame::Size4KiB(_));
            t_assert_eq!(offset, 0);
            t_assert_eq!(flags, PageTableFlags::BIT_10);
        } else {
            tfail!("expected page to be mapped");
        }
        log::info!("flags updated");

        let freed_frame = page_table.clear(page).map_err_debug_display().tunwrap()?;

        let UnmappedFrame::NotPresent { entry: freed_entry } = freed_frame else {
            tfail!("Expected to unmap not present page");
        };

        t_assert_eq!(freed_entry.addr(), fake_frame.start_address());
        t_assert_eq!(freed_entry.flags(), PageTableFlags::BIT_10);

        unsafe {
            // Safety: page no longer used
            PageAllocator::get_for_kernel().lock().free_page(page);
        }

        Ok(())
    }

    #[kernel_test]
    fn test_map_specific_pages() -> Result<(), KernelTestError> {
        const START_ADDRS: &[u64] = &[
            // this vaddr should use the same l3 page table as a readonyl
            // page from the bootloader which caused a page_fault:
            // See: https://github.com/rust-osdev/bootloader/issues/443
            0xbe296000,
        ];
        let mut pages: [Option<Page<Size4KiB>>; START_ADDRS.len()] = [None; START_ADDRS.len()];

        let mut frame_allocator = FrameAllocator::get_for_kernel().lock();
        let mut page_allocator = PageAllocator::get_for_kernel().lock();
        let mut page_table = PageTable::get_for_kernel().lock();

        for (i, p) in &mut pages.iter_mut().enumerate() {
            t_assert_eq!(&None, p);

            let vaddr = VirtAddr::new(START_ADDRS[i]);
            let page = Page::from_start_address(vaddr)
                .map_err_debug_display()
                .tunwrap()?;

            page_allocator.try_allocate_page(page).tunwrap()?;

            debug!("Map page {:?}", page);
            let frame = frame_allocator.alloc_4k().tunwrap()?;
            unsafe {
                page_table
                    .map_to_with_table_flags(
                        page,
                        frame,
                        PageTableFlags::PRESENT,
                        PageTableFlags::KERNEL_TABLE_FLAGS,
                        frame_allocator.as_mut(),
                    )
                    .map_err_debug_display()
                    .tunwrap()?
                    .flush();
            }

            *p = Some(page);
        }

        for page in &mut pages {
            let Some(page) = page.take() else {
                tfail!("page should always be some");
            };
            trace!("unmap page {:?}", page);

            let (frame, _, flush) = page_table.unmap(page).map_err_debug_display().tunwrap()?;
            flush.flush();
            unsafe {
                // Safety: page and frame unmapped and no longer accessible
                page_allocator.free_page(page);
                frame_allocator.free(frame);
            }
        }

        Ok(())
    }

    #[kernel_test]
    fn test_map_lots_of_pages() -> Result<(), KernelTestError> {
        const PAGE_COUNT: usize = 1000;
        let mut pages: [Option<Page<Size4KiB>>; PAGE_COUNT] = [None; PAGE_COUNT];

        let mut page_alloc = PageAllocator::get_for_kernel().lock();
        let mut frame_alloc = FrameAllocator::get_for_kernel().lock();
        let mut page_table = PageTable::get_for_kernel().lock();

        for (idx, p) in pages.iter_mut().enumerate() {
            t_assert_eq!(&None, p);

            let page = page_alloc.allocate_page::<Size4KiB>().tunwrap()?;
            trace!("Map page [{}] {:?}", idx, page);

            let frame = frame_alloc.alloc().tunwrap()?;

            unsafe {
                page_table.map_to_with_table_flags(
                    page,
                    frame,
                    PageTableFlags::PRESENT,
                    PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
                    frame_alloc.as_mut(),
                )
            }
            .map_err_debug_display()
            .tunwrap()?
            .flush();

            *p = Some(page);
        }

        for page in &mut pages {
            let Some(page) = page.take() else {
                tfail!("page should always be some");
            };
            trace!("unmap page {:?}", page);

            let (frame, _, flush) = page_table.unmap(page).map_err_debug_display().tunwrap()?;
            flush.flush();
            unsafe {
                // Safety: frame and page no longer accessable
                page_alloc.free_page(page);
                frame_alloc.free(frame);
            }
        }

        Ok(())
    }
}
