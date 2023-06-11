//! module containing memory implementation

pub mod frame_allocator;
pub mod kernel_heap;
pub mod page_allocator;
pub mod page_table;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::{boot_info, mem::page_table::KERNEL_PAGE_TABLE, prelude::LockCell};
use bootloader_api::{
    info::{FrameBuffer, MemoryRegionKind},
    BootInfo,
};
use core::{
    ops::{Deref, DerefMut},
    ptr::NonNull,
};
use page_table::{recursive_index, PageTableMapError, RecursivePageTableExt};
use thiserror::Error;
use volatile::{
    access::{ReadOnly, Readable, Writable},
    Volatile,
};
use x86_64::{
    registers::{control::Cr3, read_rip},
    structures::paging::{PageTable, RecursivePageTable, Translate},
    PhysAddr, VirtAddr,
};

/// Result type with [MemError]
pub type Result<T> = core::result::Result<T, MemError>;

/// enum for all memory related errors
#[derive(Error, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum MemError {
    #[error("allocator has not been initialized")]
    NotInit,
    #[error("out of memory")]
    OutOfMemory,
    #[error("out of pages")]
    OutOfPages,
    #[error("Null address is invalid")]
    NullAddress,
    #[error("Page already in use")]
    PageInUse,
    #[error("Allocation size({size:#x}) is invalid. Expected: {expected:#x}")]
    InvalidAllocSize { size: usize, expected: usize },
    #[error("Allocation alignment({align:#x}) is invalid. Expected: {expected:#x}")]
    InvalidAllocAlign { align: usize, expected: usize },
    #[error("Zero size allocation")]
    ZeroSizeAllocation,
    #[error("Pointer was not allocated by this allocator")]
    PtrNotAllocated(NonNull<u8>),
    #[error("Failed to free ptr")]
    FreeFailed(NonNull<u8>),
    #[error("Page Table map failed: {0:?}")]
    PageTableMap(PageTableMapError),
}

/// initialize memory: phys allocator, page allocator page table as well as
/// kernel heap allocator.
///
/// # Safety:
///
/// Must be called only once during kernel boot process. Requires locks and logging
pub unsafe fn init() {
    let (level_4_page_table, _) = Cr3::read();
    info!(
        "Level 4 page table at {:?} with rec index {:?}",
        level_4_page_table.start_address(),
        recursive_index()
    );

    let bootloader_page_table_vaddr = RecursivePageTable::l4_table_vaddr(recursive_index());

    // Safety: assuming the bootloader doesn't lie to us, this is the valid page table vaddr
    // and we have mutable access, because we are in the boot process of the kernel
    let bootloader_page_table: &'static mut PageTable =
        unsafe { &mut *bootloader_page_table_vaddr.as_mut_ptr() };

    let recursive_page_table = RecursivePageTable::new(bootloader_page_table)
        .expect("Page Table created by bootload must be recursive");

    let calc_pt_addr = recursive_page_table
        .translate_addr(bootloader_page_table_vaddr)
        .unwrap();

    assert_eq!(calc_pt_addr, level_4_page_table.start_address());
    trace!("bootloader page table can map it's own vaddr back to cr3 addr");

    page_table::init(recursive_page_table);

    {
        let mut recursive_page_table = KERNEL_PAGE_TABLE.lock();

        if log::log_enabled!(log::Level::Trace) {
            print_debug_info(&mut recursive_page_table, bootloader_page_table_vaddr);
        }

        frame_allocator::init(&boot_info().memory_regions);

        page_allocator::init(&mut recursive_page_table);
    }
    info!("kernel page table, page and frame allocators initialized.");

    kernel_heap::init();
}

/// Macro to map a page
///
/// ```no_run
/// # #[macro_use] extern crate wasabi-kernel;
/// # fn main() {
/// use x86_64::structures::paging::{Mapper, PageTableFlags, PhysFrame, Size4KiB};
/// let page: Page<Size4KiB> = todo!();
/// let phys_frame: PhysFrame<Size4KiB> = todo!();
/// let frame_allocator: &mut WasabiFrameAllocator<Size4KiB> = todo!();
///
/// map_page!(page, Size4KiB, PageTableFlags::WRITABLE);
/// map_page!(page, Size4KiB, PageTableFlags::WRITABLE, phys_frame);
/// map_page!(page, Size4KiB, PageTableFlags::WRITABLE, phys_frame, frame_allocator);
/// map_page!(Size4KiB, PageTableFlags::WRITABLE, phys_frame);
/// # }
/// ```
#[macro_export]
macro_rules! map_page {
    ($size: ident, $flags: expr) => {{
        use $crate::mem::page_allocator::PageAllocator;
        use $crate::mem::MemError;

        let page = PageAllocator::get_kernel_allocator()
            .lock()
            .allocate_page::<$size>();
        match page {
            Ok(page) => unsafe {
                // Safety: we are mapping a new unused page to a new unused frame,
                // so the mapping is save
                match map_page!(page, $size, $flags) {
                    Ok(_) => Ok(page),
                    Err(err) => Err(MemError::PageTableMap(err)),
                }
            },
            Err(err) => Err(err),
        }
    }};
    ($size: ident, $flags: expr, $frame: expr) => {{
        use x86_64::structures::paging::{Page, PhysFrame};
        use $crate::mem::page_allocator::PageAllocator;
        use $crate::mem::MemError;

        let page: Result<Page<$size>, MemError> = PageAllocator::get_kernel_allocator()
            .lock()
            .allocate_page::<$size>();

        let frame: PhysFrame<$size> = $frame;

        match page {
            Ok(page) => match map_page!(page, $size, $flags, frame) {
                Ok(_) => Ok(page),
                Err(err) => Err(MemError::PageTableMap(err)),
            },
            Err(err) => Err(err),
        }
    }};

    ($page: expr, $size: ident, $flags: expr) => {{
        use x86_64::structures::paging::PhysFrame;
        use $crate::mem::frame_allocator::WasabiFrameAllocator;
        use $crate::mem::page_table::PageTableMapError;

        let frame_alloc: &mut WasabiFrameAllocator<$size> =
            &mut WasabiFrameAllocator::<$size>::get_for_kernel().lock();
        let frame: Option<PhysFrame<$size>> = frame_alloc.alloc();

        frame
            .ok_or_else(|| PageTableMapError::FrameAllocationFailed)
            .map(|frame| map_page!($page, $size, $flags, frame, frame_alloc))
            .flatten()
    }};

    ($page: expr, $size: ident, $flags: expr, $frame: expr) => {{
        use $crate::mem::frame_allocator::WasabiFrameAllocator;
        let frame_alloc: &mut WasabiFrameAllocator<$size> =
            &mut WasabiFrameAllocator::<$size>::get_for_kernel().lock();
        map_page!($page, $size, $flags, $frame, frame_alloc)
    }};

    ($page: expr, $size: ident, $flags: expr, $frame: expr, $frame_alloc: expr) => {{
        use x86_64::structures::paging::{mapper::RecursivePageTable, Page, PhysFrame};
        use $crate::mem::page_table::{PageTableMapError, KERNEL_PAGE_TABLE};

        let kernel_page_table: &mut RecursivePageTable<'static> = &mut KERNEL_PAGE_TABLE.lock();

        let page: Page<$size> = $page;
        let frame: PhysFrame<$size> = $frame;
        kernel_page_table
            .map_to(page, frame, $flags, $frame_alloc)
            .map_err(|e| PageTableMapError::from(e))
            .map(|flusher| flusher.flush())
    }};
}

/// Extensiont trait for [VirtAddr]
pub trait VirtAddrExt {
    /// returns a [Volatile] that provides access to the value behind this address
    ///
    /// Safety: VirtAddr must be a valid reference
    unsafe fn as_volatile<'a, T>(self) -> Volatile<&'a T, ReadOnly>;

    /// returns a [Volatile] that provides access to the value behind this address
    ///
    /// Safety: VirtAddr must be a valid *mut* reference
    unsafe fn as_volatile_mut<'a, T>(self) -> Volatile<&'a mut T>;
}

#[allow(unsafe_op_in_unsafe_fn)]
impl VirtAddrExt for VirtAddr {
    unsafe fn as_volatile<'a, T>(self) -> Volatile<&'a T, ReadOnly> {
        trace!("new volatile at {self:p}");
        let r: &T = &*self.as_ptr();
        Volatile::new_read_only(r)
    }

    unsafe fn as_volatile_mut<'a, T>(self) -> Volatile<&'a mut T> {
        let r: &mut T = &mut *self.as_mut_ptr();
        Volatile::new(r)
    }
}

pub trait VolatileExt<R, T, A>
where
    R: Deref<Target = T>,
    T: Copy,
{
    fn update<F>(&mut self, update: F)
    where
        A: Writable + Readable,
        R: DerefMut,
        F: FnOnce(T) -> T;
}

impl<R, T, A> VolatileExt<R, T, A> for Volatile<R, A>
where
    R: Deref<Target = T>,
    T: Copy,
{
    fn update<F>(&mut self, update: F)
    where
        A: Writable + Readable,
        R: DerefMut,
        F: FnOnce(T) -> T,
    {
        let value = self.read();
        let updated = update(value);
        self.write(updated);
    }
}

/// prints out some random data about maped pages and phys frames
fn print_debug_info(
    recursive_page_table: &mut RecursivePageTable,
    bootloader_page_table_vaddr: VirtAddr,
) {
    recursive_page_table.print_all_mapped_regions(true);

    recursive_page_table
        .print_page_flags_for_vaddr(bootloader_page_table_vaddr, Some("Page Table L4"));

    recursive_page_table
        .print_page_flags_for_vaddr(VirtAddr::new(0x8000019540), Some("Kernel entry point"));
    recursive_page_table.print_page_flags_for_vaddr(
        VirtAddr::new(boot_info() as *const BootInfo as u64),
        Some("Boot Info"),
    );

    recursive_page_table.print_page_flags_for_vaddr(
        VirtAddr::new(boot_info().framebuffer.as_ref().unwrap() as *const FrameBuffer as u64),
        Some("Frame Buffer Info"),
    );

    recursive_page_table.print_page_flags_for_vaddr(
        VirtAddr::new(boot_info().framebuffer.as_ref().unwrap().buffer().as_ptr() as u64),
        Some("Frame Buffer Start"),
    );

    recursive_page_table.print_page_flags_for_vaddr(
        VirtAddr::new(*boot_info().rsdp_addr.as_ref().unwrap()),
        Some("RSDP"),
    );

    let rip = read_rip();
    recursive_page_table.print_page_flags_for_vaddr(rip, Some("RDI"));

    let memory_regions = &boot_info().memory_regions;

    info!("memory region count {}", memory_regions.len());
    assert_phys_not_available(
        recursive_page_table
            .translate_addr(bootloader_page_table_vaddr)
            .unwrap(),
        "page table",
    );

    let fb_vaddr = VirtAddr::from_ptr(boot_info().framebuffer.as_ref().unwrap().buffer().as_ptr());
    assert_phys_not_available(
        recursive_page_table.translate_addr(fb_vaddr).unwrap(),
        "framebuffer",
    );
    assert_phys_not_available(recursive_page_table.translate_addr(rip).unwrap(), "rip");
}

/// asserts that the given phys addr is not within the memory regions of the boot info
fn assert_phys_not_available(addr: PhysAddr, message: &str) {
    let avail_mem = &boot_info().memory_regions;

    let available = avail_mem.iter().any(|region| {
        region.start <= addr.as_u64()
            && region.end > addr.as_u64()
            && region.kind == MemoryRegionKind::Usable
    });
    assert!(!available, "{message}: Phys region {addr:p} is available!");
}
