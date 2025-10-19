//! NVMe io command set
//!
//! The specification documents can be found at <https://nvmexpress.org/specifications/>
//! specifically: NVM Command Set

use shared_derive::U8Enum;

mod read;
mod write;

pub use read::*;
pub use write::*;

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
