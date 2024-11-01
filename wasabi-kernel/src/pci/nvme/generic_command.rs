//! Data shared between all Command types
//!

use core::assert_matches::assert_matches;

use bit_field::BitField;
use shared_derive::U8Enum;
use static_assertions::const_assert_eq;
use thiserror::Error;
use x86_64::PhysAddr;

use super::queue::QueueIdentifier;

#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord)]
pub struct CommandIdentifier(pub(super) u16);

impl core::fmt::LowerHex for CommandIdentifier {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        u16::fmt(&self.0, f)
    }
}

impl core::fmt::UpperHex for CommandIdentifier {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        u16::fmt(&self.0, f)
    }
}

/// The size of all submission command entries
pub const SUBMISSION_COMMAND_ENTRY_SIZE: usize = size_of::<CommonCommand>();
const_assert_eq!(SUBMISSION_COMMAND_ENTRY_SIZE, 64);

/// Command layout shared by all commands
//
/// See: NVM Express Base Spec: Figure 88: Common Command Format
#[repr(C)]
#[derive(Clone, PartialEq, Eq, Default)]
pub struct CommonCommand {
    pub(super) dword0: CDW0,
    // NSID
    pub(super) namespace_ident: u32,
    pub(super) dword2: u32,
    pub(super) dword3: u32,
    pub(super) metadata_ptr: u64,
    pub(super) data_ptr: DataPtr,
    pub(super) dword10: u32,
    pub(super) dword11: u32,
    pub(super) dword12: u32,
    pub(super) dword13: u32,
    pub(super) dword14: u32,
    pub(super) dword15: u32,
}

/// The first command dword in a [CommonCommand]
///
/// this implementation is shared between all commands
///
/// See: NVM Express Base Spec: Figure 87: Command Dword 0
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct CDW0(u32);

#[allow(dead_code)]
impl CDW0 {
    pub(super) fn zero() -> Self {
        Self(0)
    }

    pub(super) fn opcode(&self) -> u8 {
        self.0.get_bits(0..=7) as u8
    }

    pub(super) fn set_opcode(&mut self, value: u8) {
        self.0.set_bits(0..=7, value as u32);
    }

    pub(super) fn fuse(&self) -> u8 {
        self.0.get_bits(8..=9) as u8
    }

    pub(super) fn set_fuse(&mut self, value: u8) {
        self.0.set_bits(8..=9, value as u32);
    }

    /// PSDT
    pub(super) fn prp_or_sgl(&self) -> PrpOrSgl {
        (self.0.get_bits(14..=15) as u8).try_into().unwrap()
    }

    pub(super) fn set_prp_or_sgl(&mut self, value: PrpOrSgl) {
        self.0.set_bits(14..=15, value as u8 as u32);
    }

    pub(super) fn command_identifier(&self) -> CommandIdentifier {
        CommandIdentifier(self.0.get_bits(16..=31) as u16)
    }

    pub(super) fn set_command_identifier(&mut self, value: CommandIdentifier) {
        self.0.set_bits(16..=31, value.0 as u32);
    }
}

/// specifies whether a command uses PRPs or SGLs for data transfer
///
/// this implementation is shared between all commands
///
/// See: NVM Express Base Spec: Figure 87: Command Dword 0
#[repr(u8)]
#[derive(Debug, U8Enum, Clone, Copy, PartialEq, Eq)]
pub enum PrpOrSgl {
    Prp = 0,
    /// Not Implemented
    ///
    /// I dont think I want to implement SGLs,
    /// they just seem more complex than PRP and I don't see a good upside.
    /// The Variant exists in case this is read as from the NVME controller
    SglMetaContiguBuffer = 0b01,
    /// Not Implemented
    ///
    /// I dont think I want to implement SGLs,
    /// they just seem more complex than PRP and I don't see a good upsite
    /// The Variant exists in case this is read as from the NVME controller
    SglIncludingMeta = 0b10,
}

#[repr(packed)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DataPtr {
    pub(super) prp_entry_1: PhysAddr,
    pub(super) prp_entry_2: PhysAddr,
}

impl Default for DataPtr {
    fn default() -> Self {
        DataPtr {
            prp_entry_1: PhysAddr::zero(),
            prp_entry_2: PhysAddr::zero(),
        }
    }
}

/// The size of all completion command entries
pub const COMPLETION_COMMAND_ENTRY_SIZE: usize = size_of::<CommonCompletionEntry>();
const_assert_eq!(COMPLETION_COMMAND_ENTRY_SIZE, 16);

/// Common layout shared by all completion entries
///
/// See: NVM Express Base Spec: Figure 90: Common Completion Queue Entry Layout
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommonCompletionEntry {
    pub(super) dword0: u32,
    pub(super) dword1: u32,
    pub(super) submission_queue_head: u16,
    pub(super) submission_queue_ident: QueueIdentifier,
    pub(super) command_ident: CommandIdentifier,
    pub(super) status_and_phase: StatusAndPhase,
}

impl CommonCompletionEntry {
    pub fn status(&self) -> CommandStatusCode {
        let status = self.status_and_phase;
        match status.status_code_type() {
            0 => {
                if let Ok(status) = GenericCommandStatus::try_from(status.status_code()) {
                    CommandStatusCode::GenericStatus(status)
                } else {
                    CommandStatusCode::UnknownGenericStatus(status.status_code())
                }
            }
            1 => CommandStatusCode::CommandSpecificStatus(status.status_code()),
            2 => CommandStatusCode::MediaAndDataIntegrityError(status.status_code()),
            3 => CommandStatusCode::PathRelatedStatus(status.status_code()),
            4..=6 => CommandStatusCode::Reserved {
                typ: status.status_code_type(),
                status: status.status_code(),
            },
            7 => CommandStatusCode::VendorSpecific(status.status_code()),
            _ => panic!("branch should be unreachable for 3bit value"),
        }
    }
}

/// Status and Phase of a [CommonCompletionEntry]
///
/// See: NVM Express Base Spec: Figure 93: Completion Queue Entry: Status Field
// This struct uses a u16. The spec combines this field, with the command identifier
// into a u32. Therefor bit 16 in the spec is represented as bit 0 in this struct.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusAndPhase(u16);

#[allow(unused)]
impl StatusAndPhase {
    pub fn phase(&self) -> bool {
        self.0.get_bit(0)
    }

    fn status_code(&self) -> u8 {
        self.0.get_bits(1..=8).try_into().unwrap()
    }

    fn status_code_type(&self) -> u8 {
        self.0.get_bits(9..=11).try_into().unwrap()
    }

    pub fn common_retry_delay(&self) -> u8 {
        self.0.get_bits(12..=13).try_into().unwrap()
    }

    pub fn more(&self) -> bool {
        self.0.get_bit(14)
    }

    pub fn do_not_retry(&self) -> bool {
        self.0.get_bit(15)
    }
}

/// StatusCode of an [CommonCompletionEntry]
///
/// See: NVM Express Base Spec: Figure 94: Status Code Type Values
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CommandStatusCode {
    #[error("Generic Command Error: {0:?}")]
    GenericStatus(GenericCommandStatus),
    #[error("Unknown Generic Command Error: {0:#x}")]
    UnknownGenericStatus(u8),
    #[error("Command specific error: {0:#x}")]
    CommandSpecificStatus(u8),
    #[error("Media and Data integrity error: {0:#x}")]
    MediaAndDataIntegrityError(u8),
    #[error("Path related error: {0:#x}")]
    PathRelatedStatus(u8),
    #[error("Reserved error: type {typ:#x}, code {status:#x}")]
    Reserved { typ: u8, status: u8 },
    #[error("Vendor specific error: {0:#x}")]
    VendorSpecific(u8),
}

impl CommandStatusCode {
    /// returns `true` if the status represents any type of error
    #[inline]
    pub fn is_err(self) -> bool {
        !self.is_success()
    }

    /// returns `true` if the status does not represents any type of error
    ///
    /// This is `true` for [GenericCommandStatus::Success]
    #[inline]
    pub fn is_success(self) -> bool {
        self == CommandStatusCode::GenericStatus(GenericCommandStatus::Success)
    }

    /// asserts that the status code does not represent any type of error.
    ///
    /// Same as [Self::is_success] but with a nicer debug print layout.
    #[inline]
    #[track_caller]
    pub fn assert_success(self) {
        assert_matches!(
            self,
            CommandStatusCode::GenericStatus(GenericCommandStatus::Success)
        );
    }
}

/// Generic Error Code of an [CommonCompletionEntry]
///
/// See: NVM Express Base Spec: Figure 95: Generic Command Status Values
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, U8Enum)]
pub enum GenericCommandStatus {
    Success = 0,
    InvalidCommandOpcode = 1,
    InvalidFieldInCommand = 2,
    CommandIdConflict = 3,
    DataTransferError = 4,
    CommandAbortedPowerLoss = 5,
    InternalError = 6,
    AbortRequested = 7,
    AbortSQDeletion = 8,
    AbortFailedFuse = 9,
    AbortMissingFues = 0xa,
    InvalidNamespaceFormat = 0xb,
    SequenceError = 0xc,
    InvalidSgl = 0xd,
    InvalidSglCount = 0xe,
    InvalidSglLength = 0xf,
    InvalidMetadataSglLength = 0x10,
    InvalidSglType = 0x11,
    InvalidUseOfControllerMemBuf = 0x12,
    InvaldPrpOffset = 0x13,
    AtomicWriteExceeded = 0x14,
    OperationDenied = 0x15,
    InvalidSglOffset = 0x16,
    // reserved 0x17
    HostIdInconsistentFormat = 0x18,
    KeepAliveExpired = 0x19,
    InvalidKeepAliveTimeout = 0x1a,
    AbortDueToPreemptAbort = 0x1b,
    SanitiizeFaild = 0x1c,
    SanitizeInProgress = 0x1d,
    InvalidSglBlockGranularity = 0x1e,
    NotSupportedForQueueInCMB = 0x1f,
    NamespaceWriteProtected = 0x20,
    Interrupted = 0x21,
    TransientTransportError = 0x22,
    ProhibitedByLockdown = 0x23,
    AdminCommandMediaNotReady = 0x24,
    // reserved 0x25 .. 0x7f
    LbaOutOfRange = 0x80,
    CapacityExceeded = 0x81,
    NamespaceNotReady = 0x82,
    ReservationConflict = 0x83,
    FormatInProgress = 0x84,
    InvalidValueSize = 0x85,
    InvalidKeySize = 0x86,
    KvKeyDoesNotExist = 0x87,
    UnrecoveredError = 0x88,
    KeyExists = 0x89,
    // Rserved 0x90 .. 0xbf
    // Vendor Specific 0xc0 .. 0xff
}
