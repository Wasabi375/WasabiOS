mod page_table;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use bootloader_api::info::FrameBuffer;
use bootloader_api::BootInfo;
use thiserror::Error;
use volatile::{access::ReadOnly, Volatile};
use x86_64::structures::paging::{RecursivePageTable, Translate};
use x86_64::VirtAddr;
use x86_64::{registers::control::Cr3, structures::paging::PageTable};

use page_table::{
    l4_table_vaddr, print_all_mapped_regions, print_page_flags_for_vaddr, recursive_index,
};

use crate::{boot_info, cpu};

#[derive(Error, Debug)]
pub enum MemError {
    #[error("out of memory")]
    OutOfMemory,
    #[error("Zero size memory block")]
    BlockSizeZero,
    #[error("Null address is invalid")]
    NullAddress,
}

pub fn init() {
    let (level_4_page_table, _) = Cr3::read();
    info!(
        "Level 4 page table at {:?} with rec index {:?}",
        level_4_page_table.start_address(),
        recursive_index()
    );

    let bootloader_page_table_vaddr = l4_table_vaddr(recursive_index());
    let bootloader_page_table: &mut PageTable =
        unsafe { &mut *bootloader_page_table_vaddr.as_mut_ptr() };

    let mut recursive_page_table = RecursivePageTable::new(bootloader_page_table)
        .expect("Page Table created by bootload must be recursive");

    let calc_pt_addr = recursive_page_table
        .translate_addr(bootloader_page_table_vaddr)
        .unwrap();

    assert_eq!(calc_pt_addr, level_4_page_table.start_address());
    trace!("bootloader page table can map it's own vaddr back to cr3 addr");

    print_all_mapped_regions(&mut recursive_page_table, true);

    print_page_flags_for_vaddr(
        &mut recursive_page_table,
        bootloader_page_table_vaddr,
        Some("Page Table L4"),
    );

    print_page_flags_for_vaddr(
        &mut recursive_page_table,
        VirtAddr::new(0x8000019540),
        Some("Kernel entry point"),
    );
    print_page_flags_for_vaddr(
        &mut recursive_page_table,
        VirtAddr::new(boot_info() as *const BootInfo as u64),
        Some("Boot Info"),
    );

    print_page_flags_for_vaddr(
        &mut recursive_page_table,
        VirtAddr::new(boot_info().framebuffer.as_ref().unwrap() as *const FrameBuffer as u64),
        Some("Frame Buffer Info"),
    );

    print_page_flags_for_vaddr(
        &mut recursive_page_table,
        VirtAddr::new(boot_info().framebuffer.as_ref().unwrap().buffer().as_ptr() as u64),
        Some("Frame Buffer Start"),
    );

    print_page_flags_for_vaddr(
        &mut recursive_page_table,
        VirtAddr::new(*boot_info().rsdp_addr.as_ref().unwrap()),
        Some("RSDP"),
    );

    let rip = unsafe { cpu::read_rip() };
    print_page_flags_for_vaddr(&mut recursive_page_table, VirtAddr::new(rip), Some("RDI"));
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
