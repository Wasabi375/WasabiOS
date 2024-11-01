//! NVMe admin command set
//!
//! The specification documents can be found at <https://nvmexpress.org/specifications/>
//! specifically: NVM Express Base Specification

use shared_derive::U8Enum;

mod features;
mod identify;
mod io_queue;

// TODO most commands take a PhysAddr or PhysFrame as an argument.
//      Using that is unsafe. Look into what functions need the unsafe keyword
//      and where I can add better abstractions

pub use features::*;
pub use identify::*;
pub use io_queue::*;

/// Opcode for the different commands
#[allow(missing_docs)]
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, U8Enum)]
pub enum CommandOpcode {
    DeleteIOSubmissionQueue = 0x0,
    CreateIOSubmissionQueue = 0x1,
    GetLogPage = 0x2,
    DeleteIOCompletionQueue = 0x4,
    CreateIOCompletionQueue = 0x5,
    Identify = 0x6,
    Abort = 0x8,
    SetFeatures = 0x9,
    GetFeatures = 0xa,
    AsynchronousEventRequest = 0xc,
    NamespaceManagment = 0xd,
    FirmwareCommit = 0x10,
    FirmwareImageDownload = 0x11,
    DeviceSelfTest = 0x14,
    NamespaceAttachemt = 0x15,
    KeepAlive = 0x18,
    DirectiveSend = 0x19,
    DirectiveReceive = 0x1a,
    VirtualizationManagement = 0x1c,
    NVMeMiSend = 0x1d,
    NVMeMIReceive = 0x1e,
    CapacityManagement = 0x20,
    Lockdown = 0x24,
    DoorbellBufferConfig = 0x7c,
    FabricsCommands = 0x7f,
    FormatNVM = 0x80,
    SecuritySend = 0x81,
    SecurityReceive = 0x82,
    Sanitize = 0x84,
    GetLBAStatus = 0x86,
}
