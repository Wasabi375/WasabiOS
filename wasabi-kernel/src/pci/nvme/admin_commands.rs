//! NVMe admin command set
//!
//! The specification documents can be found at https://nvmexpress.org/specifications/
//! specifically: NVM Express Base Specification

use bit_field::BitField;
use shared_derive::U8Enum;
use x86_64::structures::paging::PhysFrame;

use super::{CommonCommand, CDW0};

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

#[allow(missing_docs)]
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, U8Enum)]
#[non_exhaustive]
/// CNS Value
/// See: NVM Express Base Spec: Figure 274: Identify CNS Value
pub enum IdentifyNamespaceIdent {
    Namespace = 0x0,
    Controller = 0x1,
}

/// Create the [CommonCommand] data structure for an identify command
///
/// See: NVM Express Base Spec: 5.17
pub fn create_identify_command(
    cns: IdentifyNamespaceIdent,
    data_frame: PhysFrame,
) -> CommonCommand {
    let mut cdw0 = CDW0::zero();
    cdw0.set_opcode(CommandOpcode::Identify as u8);
    cdw0.set_prp_or_sgl(super::PrpOrSgl::Prp);
    // TODO command identifier: used to identify corresponding completion command

    let mut command = CommonCommand::default();
    command.dword0 = cdw0;
    let mut dword10 = 0;
    dword10.set_bits(0..=8, cns as u8 as u32);
    command.dword10 = dword10;

    match cns {
        IdentifyNamespaceIdent::Namespace => {
            todo!("set nsid for {:?} identify command", cns);
        }
        _ => {
            // NSID, CNTID and CSI not used
        }
    }

    command.data_ptr.prp_entry_1 = data_frame.start_address();
    command
}
