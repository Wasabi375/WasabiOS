//! Structs representing the different NVME Controller Properties
//!
//! The specification documents can be found at <https://nvmexpress.org/specifications/>
//! specifically: NVM Express Base Specification

// TODO temp
#![allow(missing_docs)]

use bit_field::BitField;
use shared::primitive_enum::InvalidValue;
use shared_derive::U8Enum;

/// The capabilities of the NVMe Controller
///
/// See: NVM Express Base Spec: Figure 36: Offset 0h: CAP
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// The maximum size of IO submission and completion queues.
    ///
    /// This value must be 2 or greater
    // this is a 0 based value with a minimum of 0x1, so min 2 entries
    pub maximum_queue_entries: u16,
    pub contiguous_queues_required: bool,
    pub arbitrations_supported: u8,

    reserved_1: u8,

    pub timeout: u8,
    pub doorbell_stride: u32,
    pub subsystem_reset_supported: bool,
    pub command_sets_supported: u8,
    pub boot_partition_support: bool,
    pub power_scope: u8,
    // TODO 2 ^ (12 + mpsmin)
    pub mem_min_page_size: u32,
    pub mem_max_page_size: u32,
    pub persistent_memory_region_support: bool,
    pub memory_buffer_support: bool,
    pub subsystem_shutdown_support: bool,
    pub ready_modes_support: u8,

    reserved_2: u8,
}

impl From<u64> for Capabilities {
    fn from(value: u64) -> Self {
        let maximum_queue_entries: u16 = value.get_bits(0..=15).try_into().unwrap();
        let contiguous_queues_required: bool = value.get_bit(16);
        let arbitrations_supported: u8 = value.get_bits(17..=18).try_into().unwrap();
        let reserved_1: u8 = value.get_bits(19..=23).try_into().unwrap();
        let timeout: u8 = value.get_bits(24..=31).try_into().unwrap();
        let doorbell_stride: u32 = (1 << (2 + value.get_bits(32..=35))).try_into().unwrap();
        let subsystem_reset_supported: bool = value.get_bit(36);
        let command_sets_supported: u8 = value.get_bits(37..=44).try_into().unwrap();
        let boot_partition_support: bool = value.get_bit(45);
        let power_scope: u8 = value.get_bits(46..=47).try_into().unwrap();
        let mem_min_page_size: u32 = value.get_bits(48..=51).try_into().unwrap();
        let mem_max_page_size: u32 = value.get_bits(52..=55).try_into().unwrap();
        let persistent_memory_region_support: bool = value.get_bit(56);
        let memory_buffer_support: bool = value.get_bit(57);
        let subsystem_shutdown_support: bool = value.get_bit(58);
        let ready_modes_support: u8 = value.get_bits(59..=60).try_into().unwrap();
        let reserved_2: u8 = value.get_bits(61..=63).try_into().unwrap();

        Self {
            maximum_queue_entries,
            contiguous_queues_required,
            arbitrations_supported,
            reserved_1,
            timeout,
            doorbell_stride,
            subsystem_reset_supported,
            command_sets_supported,
            boot_partition_support,
            power_scope,
            mem_min_page_size,
            mem_max_page_size,
            persistent_memory_region_support,
            memory_buffer_support,
            subsystem_shutdown_support,
            ready_modes_support,
            reserved_2,
        }
    }
}

impl Into<u64> for Capabilities {
    fn into(self) -> u64 {
        let doorbell_stride = self.doorbell_stride.ilog2() - 2;
        assert_eq!(
            self.doorbell_stride,
            1 << (doorbell_stride + 12),
            "doorbell_stride {:#x} must be a power of 2 and between 4 and 2^17",
            self.doorbell_stride
        );

        (self.maximum_queue_entries as u64)
            | (self.contiguous_queues_required as u64) << 16
            | (self.arbitrations_supported as u64) << 17
            | (self.reserved_1 as u64) << 19
            | (self.timeout as u64) << 24
            | (self.doorbell_stride as u64) << 32
            | (self.subsystem_reset_supported as u64) << 36
            | (self.command_sets_supported as u64) << 37
            | (self.boot_partition_support as u64) << 45
            | (self.power_scope as u64) << 46
            | (self.mem_min_page_size as u64) << 48
            | (self.mem_max_page_size as u64) << 52
            | (self.persistent_memory_region_support as u64) << 56
            | (self.memory_buffer_support as u64) << 57
            | (self.subsystem_shutdown_support as u64) << 58
            | (self.ready_modes_support as u64) << 59
            | (self.reserved_2 as u64) << 61
    }
}

/// The configuration of the NVMe Controller
///
/// See: NVM Express Base Spec: Figure 46: Offset 14H: CC
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerConfiguration {
    pub enable: bool,

    reserved_1: u8,

    /// 3 bit value set during initialization
    pub command_set_selected: u8,
    /// The page size used in the host memory.
    ///
    /// Queues need to be aligned to this.
    /// Min: 4KiB, Max: 128MiB
    ///
    /// Must be a power of 2
    // Calculated as 2^(12 + value)
    pub memory_page_size: u32,
    pub arbitration_mechanism: ArbitrationMechanism,
    pub shutdown_notification: u8,
    /// size of a io submission command entry
    ///
    /// Must be a power of 2
    // Calculated as 2 ^ value
    pub io_submission_queue_entry_size: u16,
    /// size of a io completion command entry
    ///
    /// Must be a power of 2
    // Calculated as 2 ^ value
    pub io_completion_queue_entry_size: u16,
    pub controller_ready_independent_of_media_enable: bool,

    reserved_2: u8,
}

impl From<u32> for ControllerConfiguration {
    fn from(value: u32) -> Self {
        let enable = value.get_bit(0);
        let reserved_1 = value.get_bits(1..=3).try_into().unwrap();
        let command_set_selected = value.get_bits(4..=6).try_into().unwrap();
        let memory_page_size = (1 << (12 + value.get_bits(7..=10))).try_into().unwrap();
        let arbitration_mechanism = value.get_bits(11..=13).try_into().unwrap();
        let shutdown_notification = value.get_bits(14..=15).try_into().unwrap();
        let io_submission_queue_entry_size = (1 << value.get_bits(16..=19)).try_into().unwrap();
        let io_completion_queue_entry_size = (1 << value.get_bits(20..23)).try_into().unwrap();
        let controller_ready_independent_of_media_enable = value.get_bit(24);
        let reserved_2 = value.get_bits(25..=31).try_into().unwrap();

        Self {
            enable,
            reserved_1,
            command_set_selected,
            memory_page_size,
            arbitration_mechanism,
            shutdown_notification,
            io_submission_queue_entry_size,
            io_completion_queue_entry_size,
            controller_ready_independent_of_media_enable,
            reserved_2,
        }
    }
}

impl Into<u32> for ControllerConfiguration {
    fn into(self) -> u32 {
        let memory_page_size = self.memory_page_size.ilog2() - 12;
        assert_eq!(
            self.memory_page_size,
            1 << (memory_page_size + 12),
            "memory_page_size {:#x} must be a power of 2 and between 4KiB and 128MiB",
            self.memory_page_size
        );

        let sub_queue_size = self.io_submission_queue_entry_size.ilog2();
        assert_eq!(
            self.io_submission_queue_entry_size,
            1 << sub_queue_size,
            "io_submission_queue_entry_size {:#x} must be a power of 2",
            self.io_submission_queue_entry_size
        );

        let comp_queue_size = self.io_completion_queue_entry_size.ilog2();
        assert_eq!(
            self.io_completion_queue_entry_size,
            1 << comp_queue_size,
            "io_completion_queue_entry_size {:#x} must be a power of 2",
            self.io_completion_queue_entry_size
        );

        (self.enable as u32)
            | (self.reserved_1 as u32) << 1
            | (self.command_set_selected as u32) << 4
            | (memory_page_size as u32) << 7
            | (self.arbitration_mechanism as u8 as u32) << 11
            | (self.shutdown_notification as u32) << 14
            | (sub_queue_size as u32) << 16
            | (comp_queue_size as u32) << 20
            | (self.controller_ready_independent_of_media_enable as u32) << 24
            | (self.reserved_2 as u32) << 25
    }
}

/// The arbitration mechanism used for command queue prioritazation
///
/// See: NVM Express Base Spec: Figure 46: Offset 14h: CC
#[repr(u8)]
#[derive(Debug, U8Enum, Clone, PartialEq, Eq)]
pub enum ArbitrationMechanism {
    RoundRobbin = 0b000,
    WeightedRoundRobbin = 0b001,
    Reserved1 = 0b010,
    Reserved2 = 0b110,
    VendorSpecific = 0b111,
}

impl TryFrom<u32> for ArbitrationMechanism {
    type Error = InvalidValue<u32>;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        let downcast: u8 = value.try_into().map_err(|_| InvalidValue { value })?;
        downcast.try_into().map_err(|_| InvalidValue { value })
    }
}

/// Status of the controller
///
/// See: NVM Express Base Spec: Figure 47: Offset 1CH: CSTS
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerStatus {
    pub ready: bool,
    pub fatal_error: bool,
    pub shutdown_status: ShutdownStatus,
    pub subsystem_reset_occured: bool,
    pub processing_paused: bool,
    pub shutdown_type: bool,
    reserved: u32,
}

impl Into<u32> for ControllerStatus {
    fn into(self) -> u32 {
        self.reserved << 7
            | (self.shutdown_type as u32) << 6
            | (self.processing_paused as u32) << 5
            | (self.subsystem_reset_occured as u32) << 4
            | (self.shutdown_status as u32) << 2
            | (self.fatal_error as u32) << 1
            | (self.ready as u32)
    }
}

impl From<u32> for ControllerStatus {
    fn from(value: u32) -> Self {
        let ready = value.get_bit(0);
        let fatal_error = value.get_bit(1);
        let shutdown_status = value
            .get_bits(2..=3)
            .try_into()
            .expect("Shutdown status should covere all 4 2bit variants");
        let subsystem_reset_occured = value.get_bit(4);
        let processing_paused = value.get_bit(5);
        let shutdown_type = value.get_bit(6);
        let reserved = value.get_bits(7..=31);

        Self {
            ready,
            fatal_error,
            shutdown_status,
            subsystem_reset_occured,
            processing_paused,
            shutdown_type,
            reserved,
        }
    }
}

/// The shutdown status of an NVME controller
///
/// See: NVM Express Base Spec: Figure 47: Offset 1Ch: CSTS
#[repr(u8)]
#[derive(Debug, U8Enum, Clone, PartialEq, Eq)]
pub enum ShutdownStatus {
    /// No shutdown requested
    NormalOperation = 0,
    Occuring = 0b01,
    Complete = 0b10,
    Reserved = 0b11,
}

impl TryFrom<u32> for ShutdownStatus {
    type Error = InvalidValue<u32>;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        let downcast: u8 = value.try_into().map_err(|_| InvalidValue { value })?;
        downcast.try_into().map_err(|_| InvalidValue { value })
    }
}

/// AQA: Admin Queue Attributes
///
/// See: NVM Express base Spec: Figure 49: Offset 24h: AQA
pub struct AdminQueueAttributes {
    /// Must not be more than 4096
    pub submission_queue_size: u16,

    /// Must not be more than 4096
    pub completion_queue_size: u16,

    reserved_1: u8,
    reserved_2: u8,
}

impl From<u32> for AdminQueueAttributes {
    fn from(value: u32) -> Self {
        let submission_queue_size = value.get_bits(0..=11) + 1;
        assert!(submission_queue_size <= 4096);
        let completion_queue_size = value.get_bits(16..=27) + 1;
        assert!(completion_queue_size <= 4096);

        let reserved_1 = value.get_bits(12..=15).try_into().unwrap();
        let reserved_2 = value.get_bits(28..=31).try_into().unwrap();

        Self {
            submission_queue_size: submission_queue_size as u16,
            completion_queue_size: completion_queue_size as u16,
            reserved_1,
            reserved_2,
        }
    }
}

impl Into<u32> for AdminQueueAttributes {
    fn into(self) -> u32 {
        // Convert into 0 based value
        let sub_value = self.submission_queue_size - 1;
        let comp_value = self.completion_queue_size - 1;

        (sub_value as u32)
            | (comp_value as u32) << 16
            | (self.reserved_1 as u32) << 12
            | (self.reserved_2 as u32) << 28
    }
}
