mod frame_allocator;
mod kernel_heap;
mod page_allocator;
mod page_table;

use core::ptr::NonNull;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::mem::frame_allocator::PhysAllocator;
use crate::mem::page_allocator::PageAllocator;
use crate::{boot_info, cpu};
use bootloader_api::info::{FrameBuffer, MemoryRegionKind};
use bootloader_api::BootInfo;
use page_table::{recursive_index, RecursivePageTableExt};
use shared::lockcell::LockCell;
use thiserror::Error;
use volatile::{access::ReadOnly, Volatile};
use x86_64::structures::paging::{RecursivePageTable, Translate};
use x86_64::{registers::control::Cr3, structures::paging::PageTable};
use x86_64::{PhysAddr, VirtAddr};

pub type Result<T> = core::result::Result<T, MemError>;

pub use page_table::KernelPageTable;

#[derive(Error, Debug)]
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
}

pub fn init() {
    let (level_4_page_table, _) = Cr3::read();
    info!(
        "Level 4 page table at {:?} with rec index {:?}",
        level_4_page_table.start_address(),
        recursive_index()
    );

    let bootloader_page_table_vaddr = RecursivePageTable::l4_table_vaddr(recursive_index());
    let bootloader_page_table: &mut PageTable =
        unsafe { &mut *bootloader_page_table_vaddr.as_mut_ptr() };

    let recursive_page_table = RecursivePageTable::new(bootloader_page_table)
        .expect("Page Table created by bootload must be recursive");

    let calc_pt_addr = recursive_page_table
        .translate_addr(bootloader_page_table_vaddr)
        .unwrap();

    assert_eq!(calc_pt_addr, level_4_page_table.start_address());
    trace!("bootloader page table can map it's own vaddr back to cr3 addr");

    KernelPageTable::get().init(recursive_page_table);
    let mut recursive_page_table = KernelPageTable::get().lock();

    if log::log_enabled!(log::Level::Trace) {
        print_debug_info(&mut recursive_page_table, bootloader_page_table_vaddr);
    }

    PhysAllocator::init(&boot_info().memory_regions);
    let _phys_alloc = PhysAllocator::get();

    PageAllocator::get_kernel_allocator()
        .lock()
        .init_from_page_table(&mut recursive_page_table);
    drop(recursive_page_table);

    kernel_heap::init();
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

    let rip = unsafe { cpu::read_rip() };
    recursive_page_table.print_page_flags_for_vaddr(VirtAddr::new(rip), Some("RDI"));

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
    assert_phys_not_available(
        recursive_page_table
            .translate_addr(VirtAddr::new(rip))
            .unwrap(),
        "rip",
    );
}

fn assert_phys_not_available(addr: PhysAddr, message: &str) {
    let avail_mem = &boot_info().memory_regions;

    let available = avail_mem.iter().any(|region| {
        region.start <= addr.as_u64()
            && region.end > addr.as_u64()
            && region.kind == MemoryRegionKind::Usable
    });
    assert!(!available, "{message}: Phys region {addr:p} is available!");
}

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
