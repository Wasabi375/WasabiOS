use core::{mem::size_of, ptr::addr_of, str::from_utf8};

use static_assertions::const_assert_eq;
use x86_64::PhysAddr;

use crate::mem::ptr::UntypedPtr;

use super::AcpiError;

#[allow(unused_imports)]
use log::trace;

/// Calculates the u8 sum, using wrapping addition
///
/// # Safety
///
/// `start` to `start + (length -1)` must be safe to dereference
unsafe fn wrapping_sum<T: ?Sized>(start: &T, length: usize) -> u8 {
    let mut sum = 0u8;
    let mut ptr: *const u8 = start as *const T as *const u8;
    for _ in 0..length {
        unsafe {
            // Safety: see function definition
            sum = sum.wrapping_add(*ptr);
            ptr = ptr.offset(1);
        }
    }
    sum
}

/// ACPI table header
#[repr(packed)]
pub struct Header {
    pub signature: [u8; 4],
    /// length in bytes of the entire table
    pub length: u32,
    /// must be 1
    pub revision: u8,
    pub checksum: u8,
    pub oemid: [u8; 6],
    pub oem_table_id: u64,
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}
const HEADER_SIZE: usize = 36;
const_assert_eq!(HEADER_SIZE, size_of::<Header>());

impl Header {
    pub fn sig_utf8(&self) -> &str {
        from_utf8(&self.signature).expect("failed to parse signature as utf8")
    }

    pub fn verify(&self, checksum_err_msg: &'static str) -> Result<(), AcpiError> {
        // Safety: we have to assume that the supplied length is valid
        if unsafe { wrapping_sum(self, self.length as usize) } != 0 {
            return Err(AcpiError::InvalidChecksum(checksum_err_msg));
        }

        return Ok(());
    }
}

/// Extended System Descriptor Table
pub struct XSDT {
    // signature must be "XSDT"
    // revision must be 1
    pub header: &'static Header,
    /// number of entries
    pub entry_count: usize,
    /// this ptr might not be aligned
    pub first_entry: *const PhysAddr,
}

impl XSDT {
    /// converts vaddr to [XSDT] and checks that it is valid
    ///
    /// # Safety vaddr must point to valid XSDT struct
    pub unsafe fn from_ptr(ptr: UntypedPtr) -> Result<Self, AcpiError> {
        // see function definition
        let header: &Header = unsafe { ptr.as_ref() };
        header.verify("XSDT")?;
        if header.signature != *b"XSDT" {
            return Err(AcpiError::InvalidSignature("XSDT"));
        }

        if header.revision != 1 {
            return Err(AcpiError::UnexpectedRevision("XSDT rev 1 expected"));
        }

        let first_entry = addr_of!(*header).wrapping_add(1) as *const PhysAddr;
        let entry_count = (header.length as usize - HEADER_SIZE) / size_of::<PhysAddr>();

        Ok(Self {
            header,
            entry_count,
            first_entry,
        })
    }

    pub fn entry(&self, index: usize) -> Option<PhysAddr> {
        if index >= self.entry_count {
            return None;
        }
        // Safety: we just checked that we are within bounds
        let paddr = unsafe { core::ptr::read_unaligned(self.first_entry.wrapping_add(index)) };
        Some(paddr)
    }

    pub fn entries(&self) -> impl Iterator<Item = PhysAddr> + '_ {
        (0..self.entry_count).map(|i| self.entry(i).unwrap())
    }
}

pub enum AcpiTable {
    Unknown(&'static Header),
}

impl AcpiTable {
    /// converts vaddr to [AcpiTable] and checks that it is valid
    ///
    /// # Safety vaddr must point to valid [Header] struct followed by its data
    pub unsafe fn from_ptr(ptr: UntypedPtr) -> Result<Self, AcpiError> {
        // see function definition
        let header: &Header = unsafe { ptr.as_ref() };
        header.verify("XSDT entry")?;

        let this = match header.signature {
            _ => AcpiTable::Unknown(header),
        };
        Ok(this)
    }
}

/// Root System Descriptor Pointer
#[derive(Debug)]
#[repr(packed)]
pub struct RsdpV1 {
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oemid: [u8; 6],
    pub revision: u8,
    pub rsdt_addr: u32,
}
const_assert_eq!(size_of::<RsdpV1>(), 20);

impl RsdpV1 {
    pub fn verify(&self) -> Result<(), AcpiError> {
        const SIGNATURE: [u8; 8] = *b"RSD PTR ";
        if self.signature != SIGNATURE {
            return Err(AcpiError::InvalidSignature("RSDP"));
        }

        // Safety: self is 20 bytes in size
        if unsafe { wrapping_sum(self, 20) } != 0 {
            return Err(AcpiError::InvalidChecksum("RSDP rev 1"));
        }

        if self.revision > 2 {
            return Err(AcpiError::UnexpectedRevision(
                "RSDP revision is not 0, 1 or 2",
            ));
        }

        Ok(())
    }
}

#[derive(Debug)]
#[repr(packed)]
pub struct RsdpV2 {
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oemid: [u8; 6],
    pub revision: u8,
    pub rsdt_addr: u32,
    pub length: u32,
    pub xsdt_addr: u64,
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}
const_assert_eq!(size_of::<RsdpV2>(), 36);

impl RsdpV2 {
    pub fn as_rev1(&self) -> &RsdpV1 {
        unsafe {
            // Safety: rev 1 and rev 2 share the same layout for the fields in v1
            &*(self as *const RsdpV2 as *const RsdpV1)
        }
    }

    pub fn verify(&self) -> Result<(), AcpiError> {
        self.as_rev1().verify()?;

        // Safety: self is 36 bytes in size
        if unsafe { wrapping_sum(self, 36) } != 0 {
            return Err(AcpiError::InvalidChecksum("ACPI rev 2"));
        }

        if self.revision != 2 {
            return Err(AcpiError::UnexpectedRevision("RSDP revision must be 2"));
        }

        Ok(())
    }
}
