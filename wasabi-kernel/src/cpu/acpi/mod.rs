//! Acpi(Advanced Cnfiguration and Power Interface) structs and utilities
//!
#![allow(dead_code)] // TODO temp

mod structs;

use core::str::from_utf8;

use hashbrown::HashMap;
use thiserror::Error;
use x86_64::{
    structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB},
    PhysAddr, VirtAddr,
};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::{
    cpu::acpi::structs::{Header, RsdpV1, XSDT},
    map_frame,
    mem::MemError,
    utils::log_hex_dump,
};

use self::structs::{AcpiTable, RsdpV2};

#[derive(Debug, Error)]
#[allow(missing_docs)]
pub enum AcpiError {
    #[error("Invalid checksum for {0}")]
    InvalidChecksum(&'static str),
    #[error("Invalid signature for {0}")]
    InvalidSignature(&'static str),
    #[error("Unexpected data in table: {0}")]
    UnexpectedRevision(&'static str),
    #[error("Memory error {0}")]
    Mem(#[from] MemError),
    #[error("This hardware is not supported: {0}")]
    UnsupportedHardware(&'static str),
}

/// Advanced Configuration and Power Interface
pub struct ACPI {
    rsdp: &'static RsdpV2,
    mappings: HashMap<PhysFrame, Page>,
}

impl ACPI {
    /// reads the rsdp table structure to parse the ACPI data
    pub fn from_rsdp(rsdp_paddr: PhysAddr) -> Result<Self, AcpiError> {
        info!("init ACPI");
        let frame = PhysFrame::containing_address(rsdp_paddr);
        let offset = rsdp_paddr - frame.start_address();

        let page = unsafe {
            // TODO this should be done in the macro
            use shared::sync::lockcell::LockCell;
            // Safety: we are the only code mapping this frame
            map_frame!(
                Size4KiB,
                PageTableFlags::PRESENT | PageTableFlags::NO_CACHE | PageTableFlags::NO_EXECUTE,
                frame
            )
        }?;

        let vaddr = page.start_address() + offset;

        let mut mappings = HashMap::new();
        mappings.insert(frame, page);

        let rsdp_v1: &RsdpV1 = unsafe {
            // Safety: we need to dereference this field, to check if it is actually valid
            // this is inherently unsafe
            &*vaddr.as_ptr()
        };

        rsdp_v1.verify()?;
        if rsdp_v1.revision != 2 {
            return Err(AcpiError::UnsupportedHardware("We require RSDP revision 2"));
        }
        let rsdp: &RsdpV2 = unsafe {
            // Safety: we checked and this is of type RSDP revision 2
            &*vaddr.as_ptr()
        };
        rsdp.verify()?;

        if let Ok(oemid) = from_utf8(rsdp.oemid.as_slice()) {
            info!("RSDP OemId: {}", oemid);
        } else {
            info!("RSDP OemId: {:?}", rsdp.oemid);
        }

        let mut this = Self { rsdp, mappings };

        let xsdt_vaddr = this.phys_to_virt(PhysAddr::new(rsdp.xsdt_addr))?;
        let xsdt = unsafe {
            // Safety: pyhs_to_virt ensures that vaddr is mapped
            XSDT::from_vaddr(xsdt_vaddr)?
        };

        info!("XSDT: entries {}", xsdt.entry_count);
        unsafe {
            // Safety: xsdt is mapped
            log_hex_dump(
                "XSDT: ",
                log::Level::Debug,
                module_path!(),
                xsdt_vaddr,
                xsdt.header.length as usize,
            );

            log_hex_dump(
                "XSDT entries",
                log::Level::Debug,
                module_path!(),
                xsdt_vaddr + 36,
                (xsdt.header.length - 36) as usize,
            );
        }

        for entry in this.iter_xsdt(&xsdt) {
            match entry {
                Ok(AcpiTable::Unknown(header)) => {
                    info!(
                        "unknown table entry: {}, {:?} at {:p}",
                        header.sig_utf8(),
                        header.signature,
                        header as *const Header
                    )
                }
                Err(err) => error!("failed to parse xsdt entry: {err}"),
            }
        }

        Ok(this)
    }

    fn iter_xsdt<'a>(
        &'a mut self,
        xsdt: &'a XSDT,
    ) -> impl Iterator<Item = Result<AcpiTable, AcpiError>> + 'a {
        xsdt.entries()
            .map(|paddr| self.phys_to_virt(paddr).expect("failed to map acpi table"))
            // Safety: phys_to_virt maps the vaddr and we get the paddr
            // from a valid xsdt table
            .map(|vaddr| unsafe { AcpiTable::from_vaddr(vaddr) })
    }

    fn phys_to_virt(&mut self, paddr: PhysAddr) -> Result<VirtAddr, MemError> {
        let frame = PhysFrame::containing_address(paddr);
        trace!("phys to virt: {:p}", paddr);
        let page: Page<_> = if let Some(page) = self.mappings.get(&frame) {
            *page
        } else {
            unsafe {
                // TODO this should be done in the macro
                use shared::sync::lockcell::LockCell;
                // Safety: we are the only code mapping this frame
                map_frame!(
                    Size4KiB,
                    PageTableFlags::PRESENT | PageTableFlags::NO_CACHE | PageTableFlags::NO_EXECUTE,
                    frame
                )
            }?
        };
        let offset = paddr - frame.start_address();
        Ok(page.start_address() + offset)
    }
}
