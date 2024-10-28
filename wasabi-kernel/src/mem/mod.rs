//! # Memory and Type usage in the kernel
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

pub mod frame_allocator;
pub mod kernel_heap;
pub mod page_allocator;
pub mod page_table;
pub mod page_table_debug_ext;
pub mod ptr;
pub mod structs;

use log::Level;
#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use ptr::UntypedPtr;

use crate::{
    kernel_info::KernelInfo,
    mem::{page_table::KERNEL_PAGE_TABLE, page_table_debug_ext::PageTableDebugExt},
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
    structures::paging::{PageTable, RecursivePageTable, Translate},
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
    PageTableMap(PageTableMapError),
}

impl From<PageTableMapError> for MemError {
    fn from(value: PageTableMapError) -> Self {
        Self::PageTableMap(value)
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

/// Macro to map a frame
///
/// ```no_run
/// # #[macro_use] extern crate wasabi-kernel;
/// # fn main() {
/// # use x86_64::structures::paging::{Mapper, PageTableFlags, PhysFrame, Size4KiB};
/// # let phys_frame: PhysFrame<Size4KiB> = todo!();
///
/// let page = map_frame!(Size4KiB, PageTableFlags::WRITABLE | PAGE_TABLE_FLAGS::PRESENT, phys_frame)?;
/// let (page, frame) = map_frame!(Size4KiB, PageTableFlags::WRITABLE | PAGE_TABLE_FLAGS::PRESENT)?;
/// # }
/// ```
///
/// TODO Safety
#[macro_export]
macro_rules! map_frame {
    ($size: ident, $flags: expr, $frame: expr) => {{
        #[allow(unused_imports)]
        use $crate::map_page;

        let page: Result<x86_64::structures::paging::Page<$size>, $crate::mem::MemError> =
            $crate::mem::page_allocator::PageAllocator::get_kernel_allocator()
                .lock() // TODO call lock via full path
                .allocate_page::<$size>();

        let frame: x86_64::structures::paging::PhysFrame<$size> = $frame;

        match page {
            Ok(page) => match map_page!(page, $size, $flags, frame) {
                Ok(_) => Ok(page),
                Err(err) => Err($crate::mem::MemError::PageTableMap(err)),
            },
            Err(err) => Err(err),
        }
    }};
    ($size: ident, $flags: expr) => {{
        let frame: Option<x86_64::structures::paging::PhysFrame<$size>> =
            $crate::mem::frame_allocator::WasabiFrameAllocator::get_for_kernel()
                .lock()
                .alloc::<$size>();
        match frame {
            // Safety: we allocated both the frame and page
            Some(frame) => unsafe { map_frame!($size, $flags, frame) }.map(|page| (page, frame)),
            None => Err($crate::mem::MemError::OutOfMemory),
        }
    }};
}

/// Macro to map a page. Returns `Result<Page, PageTableMapError>`
///
/// # Example
///
/// ```no_run
/// # #[macro_use] extern crate wasabi-kernel;
/// # fn main() {
/// use x86_64::structures::paging::{Mapper, PageTableFlags, PhysFrame, Size4KiB};
/// let page: Page<Size4KiB> = todo!();
/// let phys_frame: PhysFrame<Size4KiB> = todo!();
/// let frame_allocator: &mut WasabiFrameAllocator<Size4KiB> = todo!();
///
// TODO document return types
//
/// let page = map_page!(Size4KiB, PageTableFlags::WRITABLE | PageTableFlags::PRESENT)?;
/// let page = map_page!(page, Size4KiB, PageTableFlags::WRITABLE | PageTableFlags::PRESENT)?;
/// let page = map_page!(page, Size4KiB, PageTableFlags::WRITABLE | PageTableFlags::PRESENT, phys_frame)?;
/// let page = map_page!(page, Size4KiB, PageTableFlags::WRITABLE | PageTableFlags::PRESENT, phys_frame, frame_allocator)?;
/// let page = map_page!(Size4KiB, PageTableFlags::WRITABLE | PageTbaleFlags::PRESENT, phys_frame)?;
/// # }
/// ```
///
/// TODO Safety
///
/// # See
///
/// [Page](x86_64::structures::paging::Page), [PageTableMapError],
/// [PageSize](x86_64::structures::paging::PageSize),
/// [PageTableFlags], [PageTableKernelFlags], [PageAllocator],
/// [WasabiFrameAllocator]
#[macro_export]
macro_rules! map_page {
    ($size: ident, $flags: expr) => {{
        let page = $crate::mem::page_allocator::PageAllocator::get_kernel_allocator()
            .lock()
            .allocate_page::<$size>();
        match page {
            Ok(page) => unsafe {
                // Safety: we are mapping a new unused page to a new unused frame,
                // so the mapping is save
                match map_page!(page, $size, $flags) {
                    Ok(_) => Ok(page),
                    Err(err) => Err($crate::mem::MemError::PageTableMap(err)),
                }
            },
            Err(err) => Err(err),
        }
    }};

    ($page: expr, $size: ident, $flags: expr) => {{
        let frame_alloc: &mut $crate::mem::frame_allocator::WasabiFrameAllocator =
            &mut $crate::mem::frame_allocator::WasabiFrameAllocator::get_for_kernel().lock();
        let frame: Option<x86_64::structures::paging::PhysFrame<$size>> =
            frame_alloc.alloc::<$size>();

        frame
            .ok_or_else(|| $crate::mem::page_table::PageTableMapError::FrameAllocationFailed)
            .map(|frame| map_page!($page, $size, $flags, frame, frame_alloc))
            .flatten()
    }};

    ($page: expr, $size: ident, $flags: expr, $frame: expr) => {{
        let frame_alloc: &mut $crate::mem::frame_allocator::WasabiFrameAllocator =
            &mut $crate::mem::frame_allocator::WasabiFrameAllocator::get_for_kernel().lock();
        map_page!($page, $size, $flags, $frame, frame_alloc)
    }};

    ($page: expr, $size: ident, $flags: expr, $frame: expr, $frame_alloc: expr) => {{
        let kernel_page_table: &mut x86_64::structures::paging::mapper::RecursivePageTable<
            'static,
        > = &mut $crate::mem::page_table::KERNEL_PAGE_TABLE.lock();

        let page: x86_64::structures::paging::Page<$size> = $page;
        let frame: x86_64::structures::paging::PhysFrame<$size> = $frame;

        let table_flags: x86_64::structures::paging::page_table::PageTableFlags =
            x86_64::structures::paging::page_table::PageTableFlags::PRESENT
                | x86_64::structures::paging::page_table::PageTableFlags::WRITABLE;

        x86_64::structures::paging::Mapper::map_to_with_table_flags(
            kernel_page_table,
            page,
            frame,
            $flags,
            table_flags,
            $frame_alloc,
        )
        .map_err(|e| $crate::mem::page_table::PageTableMapError::from(e))
        .map(|flusher| flusher.flush())
    }};
}

/// Macro to map a page. Returns `Result<(PhysFrame, PageTableFlags), PageTableMapError>`
///
/// # Example
///
/// ```no_run
/// # #[macro_use] extern crate wasabi-kernel;
/// # fn main() {
/// use x86_64::structures::paging::{Mapper, PageTableFlags, PhysFrame, Size4KiB};
/// let page: Page<Size4KiB> = todo!();
///
// TODO document return types
//
/// let result: Result<(phys_frame, flags), PageTableMapError> = unmap_page!(page);
/// # }
/// ```
#[macro_export]
macro_rules! unmap_page {
    ($page: expr) => {{
        let kernel_page_table: &mut x86_64::structures::paging::mapper::RecursivePageTable<
            'static,
        > = &mut $crate::mem::page_table::KERNEL_PAGE_TABLE.lock();

        x86_64::structures::paging::Mapper::unmap(kernel_page_table, $page)
            .map_err(|err| {
                <$crate::mem::page_table::PageTableMapError as From<
                    x86_64::structures::paging::mapper::UnmapError,
                >>::from(err)
            })
            .map(|(frame, flags, flush)| {
                flush.flush();
                (frame, flags)
            })
    }};
}

/// Macro to free a page.
///
/// # Example
///
/// ```no_run
/// # #[macro_use] extern crate wasabi-kernel;
/// # fn main() {
/// use x86_64::structures::paging::{Mapper, PageTableFlags, PhysFrame, Size4KiB};
/// let frame: PhysFrame<Size4KiB> = todo!();
///
// TODO Saftey
//
/// free_frame!(frame);
/// # }
/// ```
#[macro_export]
macro_rules! free_page {
    ($page: expr) => {{
        $crate::mem::page_allocator::PageAllocator::get_kernel_allocator()
            .lock()
            .free_page($page);
    }};
}

/// Macro to free a frame.
///
/// # Example
///
/// ```no_run
/// # #[macro_use] extern crate wasabi-kernel;
/// # fn main() {
/// use x86_64::structures::paging::{Mapper, PageTableFlags, PhysFrame, Size4KiB};
/// let frame: PhysFrame<Size4KiB> = todo!();
///
// TODO Saftey
//
/// free_frame!(frame);
/// # }
/// ```
#[macro_export]
macro_rules! free_frame {
    ($size:ident, $frame: expr) => {{
        let frame_alloc: &mut $crate::mem::frame_allocator::WasabiFrameAllocator =
            &mut $crate::mem::frame_allocator::WasabiFrameAllocator::get_for_kernel().lock();
        frame_alloc.free::<$size>($frame);
    }};
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
