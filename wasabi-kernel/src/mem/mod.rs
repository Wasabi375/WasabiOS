//! # Memory and Type usage in the kernel
//!
//! ## Locks
//!
//! There are 4 locks that are used for kernel memory managment.
//!  1. Page Allocator
//!  2. Frame Allocator
//!  3. Page Table
//!  4. Kernel Heap
//!
//! Generally it should never be necessary to hold the kernel heap lock.
//! Any rust allocation will automatically lock the heap when necessary.
//! The other 3 locks should be held only for as long as necessary and
//! when possible be locked in the above order to prevent deadlocks.
//!
//! ## Pointer Types
//!
//! * **Memory Mapped Pointers**:
//!     In places where it is necessary to store memory mapped pointers,
//!     when possible [NonNull] should be prefered to [VirtAddr].
//!     Optional/Nullable pointers should be of type `Option<NonNull<T>>`.
//! * **Untyped Mapped Pointers**
//!     For untyped ptrs, e.g. stack or heap pointers, [UntypedPtr] should
//!     be used. Again Optional/Nullable pointers should be of type `Option<UntypedPtr>`
//! * **Unmapped Memory**:
//!     When dealing with fixed static addresses that may not be mapped to physical
//!     memory [VirtAddr] should be used.
//! * **Physical Memory**:
//!     [PhysAddr] is used to refer to physicall addresses.
//!
//! ## Data Structures
//!
//! Sometimes it is usefull to link the lifetime of a memory mapping to the lifetime
//! of a data structure, e.g. when reading a file the lifetime of that mapping should
//! match the lifetime of the file object.
//!
//! Data Structures that own a memory mapping should contain the following:
//! 1. The page(s) that are mapped. This also includes any guard pages that are mapped to
//!    non-physical memory.
//! 2. The frame(s) that any pages are mapped to. In case of guard pages, there is no need to store
//!    the guard frame (if any exists).
//! 3. Non-reference pointers into the page(s) should be of type [NonNull] and not [VirtAddr].
//!
//! [UntypedPtr]: ptr::UntypedPtr

#[cfg(feature = "mem-stats")]
pub mod stats;

pub mod frame_allocator;
pub mod kernel_heap;
pub mod page_allocator;
pub mod page_table;
pub mod page_table_debug_ext;
pub mod ptr;
pub mod structs;

use core::alloc::AllocError;

use log::Level;
#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use ptr::UntypedPtr;

use crate::{
    kernel_info::KernelInfo,
    mem::{page_table::PageTable, page_table_debug_ext::PageTableDebugExt},
    prelude::LockCell,
};
use bootloader_api::{
    info::{FrameBuffer, MemoryRegionKind},
    BootInfo,
};
use page_table::{recursive_index, PageTableMapError, RecursivePageTableExt};
use thiserror::Error;
use x86_64::{
    registers::{control::Cr3, read_rip},
    structures::paging::{
        mapper::{MapToError, UnmapError},
        PageTable as X86PageTable, RecursivePageTable, Size1GiB, Size2MiB, Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};
/// Result type with [MemError]
pub type Result<T> = core::result::Result<T, MemError>;

/// enum for all memory related errors
#[derive(Error, Debug, PartialEq, Eq, Clone)]
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
    #[error("Pointer({0:p}) was not allocated by this allocator")]
    PtrNotAllocated(UntypedPtr),
    #[error("Failed to free ptr: {0:p}")]
    FreeFailed(UntypedPtr),
    #[error("Page Table map failed: {0:?}")]
    PageTableMap(#[from] PageTableMapError),
    #[error("Heap allocation failed")]
    AllocError,
}

impl From<AllocError> for MemError {
    fn from(_value: AllocError) -> Self {
        MemError::AllocError
    }
}

impl From<MapToError<Size4KiB>> for MemError {
    fn from(value: MapToError<Size4KiB>) -> Self {
        PageTableMapError::from(value).into()
    }
}

impl From<MapToError<Size2MiB>> for MemError {
    fn from(value: MapToError<Size2MiB>) -> Self {
        PageTableMapError::from(value).into()
    }
}

impl From<MapToError<Size1GiB>> for MemError {
    fn from(value: MapToError<Size1GiB>) -> Self {
        PageTableMapError::from(value).into()
    }
}

impl From<UnmapError> for MemError {
    fn from(value: UnmapError) -> Self {
        PageTableMapError::from(value).into()
    }
}

/// initialize memory: phys allocator, page allocator page table as well as
/// kernel heap allocator.
///
/// # Safety:
///
/// Must be called only once during kernel boot process on bsp. Requires locks and logging
pub unsafe fn init() {
    let (level_4_page_table, _) = Cr3::read();

    let bootloader_page_table_vaddr = RecursivePageTable::l4_table_vaddr(recursive_index());

    info!(
        "Level 4 page table at (phys: {:p}, virt: {:p}) with rec index {:?}",
        level_4_page_table.start_address(),
        bootloader_page_table_vaddr,
        recursive_index()
    );

    // Safety: assuming the bootloader doesn't lie to us, this is the valid page table vaddr
    // and we have mutable access, because we are in the boot process of the kernel
    let bootloader_page_table: &'static mut X86PageTable =
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
        let mut recursive_page_table = PageTable::get_for_kernel().lock();

        if log::log_enabled!(log::Level::Debug) {
            print_debug_info(
                &mut recursive_page_table,
                bootloader_page_table_vaddr,
                log::Level::Debug,
            );
        }

        frame_allocator::init(&KernelInfo::get().boot_info.memory_regions);

        page_allocator::init(&mut recursive_page_table);
    }
    info!("kernel page table, page and frame allocators initialized.");

    kernel_heap::init();
    info!("mem init done!");
}

/// Calculate the number of pages required for `bytes` memory.
///
/// # Example
///
/// ```no_run
/// # #[macro_use] extern crate wasabi-kernel;
/// # fn main() {
/// # use x86_64::structures::paging::Size4KiB;
/// # use static_assertions::const_assert_eq;
/// const_assert_eq!(1, pages_required_for!(Size4KiB, 4096));
/// const_assert_eq!(1, pages_required_for!(Size4KiB, 8));
/// const_assert_eq!(2, pages_required_for!(Size4KiB, 4097));
/// const_assert_eq!(1, pages_required_for!(size: 4096, size_of::<u32>() * 5));
/// # }
/// ```
#[macro_export]
macro_rules! pages_required_for {
    ($size:ident, $bytes:expr) => {
        crate::pages_required_for!(size: <$size as x86_64::structures::paging::PageSize>::SIZE, $bytes)
    };
    (size: $size:expr, $bytes:expr) => {
        ($bytes as u64 / $size as u64)
            + if ($bytes as u64 % $size as u64 != 0) {
                1
            } else {
                0
            }
    };
}

/// Calculate the number of frames required for `bytes` memory.
///
/// # Example
///
/// ```no_run
/// # #[macro_use] extern crate wasabi-kernel;
/// # fn main() {
/// # use x86_64::structures::paging::Size4KiB;
/// # use static_assertions::const_assert_eq;
/// const_assert_eq!(1, frames_required_for!(Size4KiB, 4096));
/// const_assert_eq!(1, frames_required_for!(Size4KiB, 8));
/// const_assert_eq!(2, frames_required_for!(Size4KiB, 4097));
/// const_assert_eq!(1, frames_required_for!(4096, size_of::<u32>() * 5));
/// # }
/// ```
#[macro_export]
macro_rules! frames_required_for {
    ($size:ident, $bytes:expr) => {
        crate::pages_required_for!($size, $bytes)
    };
    (size: $size:literal, $bytes:expr) => {
        crate::pages_required_for!($size, $bytes)
    };
}

/// prints out some random data about maped pages and phys frames
fn print_debug_info(
    recursive_page_table: &mut RecursivePageTable,
    bootloader_page_table_vaddr: VirtAddr,
    level: log::Level,
) {
    recursive_page_table.print_all_mapped_regions(true, Level::Info);

    // TODO this is unsafe
    let boot_info = &KernelInfo::get().boot_info;

    recursive_page_table.print_page_flags_for_vaddr(
        bootloader_page_table_vaddr,
        level,
        Some("Page Table L4"),
    );

    recursive_page_table.print_page_flags_for_vaddr(
        VirtAddr::new(*boot_info as *const BootInfo as u64),
        level,
        Some("Boot Info"),
    );

    recursive_page_table.print_page_flags_for_vaddr(
        VirtAddr::new(boot_info.framebuffer.as_ref().unwrap() as *const FrameBuffer as u64),
        level,
        Some("Frame Buffer Info"),
    );

    recursive_page_table.print_page_flags_for_vaddr(
        VirtAddr::new(boot_info.framebuffer.as_ref().unwrap().buffer().as_ptr() as u64),
        level,
        Some("Frame Buffer Start"),
    );

    recursive_page_table.print_page_flags_for_vaddr(
        VirtAddr::new(*boot_info.rsdp_addr.as_ref().unwrap()),
        level,
        Some("RSDP"),
    );

    if let Some(framebuffer) = boot_info.framebuffer.as_ref() {
        recursive_page_table.print_page_flags_for_vaddr(
            VirtAddr::new(framebuffer as *const FrameBuffer as u64),
            level,
            Some("Frame Buffer Info"),
        );
        recursive_page_table.print_page_flags_for_vaddr(
            VirtAddr::new(framebuffer.buffer().as_ptr() as u64),
            level,
            Some("Frame Buffer Start"),
        );
    }

    let rip = read_rip();
    recursive_page_table.print_page_flags_for_vaddr(rip, level, Some("RIP"));

    let memory_regions = &boot_info.memory_regions;

    log::log!(level, "memory region count {}", memory_regions.len());
    assert_phys_not_available(
        recursive_page_table
            .translate_addr(bootloader_page_table_vaddr)
            .unwrap(),
        "page table",
    );

    let fb_vaddr = VirtAddr::from_ptr(boot_info.framebuffer.as_ref().unwrap().buffer().as_ptr());
    assert_phys_not_available(
        recursive_page_table.translate_addr(fb_vaddr).unwrap(),
        "framebuffer",
    );
    assert_phys_not_available(recursive_page_table.translate_addr(rip).unwrap(), "rip");
}

/// asserts that the given phys addr is not within the memory regions of the boot info
fn assert_phys_not_available(addr: PhysAddr, message: &str) {
    let avail_mem = &KernelInfo::get().boot_info.memory_regions;

    let available = avail_mem.iter().any(|region| {
        region.start <= addr.as_u64()
            && region.end > addr.as_u64()
            && region.kind == MemoryRegionKind::Usable
    });
    assert!(!available, "{message}: Phys region {addr:p} is available!");
}

#[cfg(feature = "test")]
mod test {
    use core::mem::size_of;

    use crate::pages_required_for;
    use testing::{kernel_test, t_assert_eq, KernelTestError};
    use x86_64::structures::paging::Size4KiB;

    #[kernel_test]
    pub fn test_pages_required_for() -> Result<(), KernelTestError> {
        t_assert_eq!(1, pages_required_for!(Size4KiB, 4096));
        t_assert_eq!(1, pages_required_for!(Size4KiB, 8));
        t_assert_eq!(2, pages_required_for!(Size4KiB, 4097));
        t_assert_eq!(1, pages_required_for!(size: 4096, size_of::<u32>() * 5));

        Ok(())
    }
}
