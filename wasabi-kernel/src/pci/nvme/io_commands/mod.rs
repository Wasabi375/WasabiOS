//! NVMe io command set
//!
//! The specification documents can be found at https://nvmexpress.org/specifications/
//! specifically: NVM Command Set

use core::fmt::{Debug, LowerHex, UpperHex};

use shared_derive::U8Enum;

mod read;

pub use read::*;

/// Opcode for the different commands
#[allow(missing_docs)]
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, U8Enum)]
pub enum CommandOpcode {
    Flush = 0x0,
    Write = 0x1,
    Read = 0x2,
    WriteUncorrectable = 0x4,
    Compare = 0x5,
    WriteZeroes = 0x8,
    DatasetManagment = 0x9,
    Verify = 0xc,
    ReservationRegister = 0xd,
    ReservationReport = 0xe,
    ReservationAcquire = 0x11,
    ReservationRelease = 0x15,
    Copy = 0x19,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LBA(u64);

impl LBA {
    pub fn new(addr: u64) -> Self {
        Self(addr)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

impl From<u64> for LBA {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl Into<u64> for LBA {
    fn into(self) -> u64 {
        self.0
    }
}

impl Debug for LBA {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LBA")
            .field_with("address", |f| f.write_fmt(format_args!("{:#x}", self.0)))
            .finish()
    }
}

impl LowerHex for LBA {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        LowerHex::fmt(&self.0, f)
    }
}

impl UpperHex for LBA {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        UpperHex::fmt(&self.0, f)
    }
}
