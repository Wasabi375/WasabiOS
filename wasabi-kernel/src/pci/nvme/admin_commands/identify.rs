use core::marker::PhantomData;

use bit_field::BitField;
use bitflags::bitflags;
use shared_derive::U8Enum;
use x86_64::{
    structures::paging::{Page, PhysFrame, Size4KiB},
    VirtAddr,
};

use crate::pci::nvme::{
    generic_command::{PrpOrSgl, CDW0},
    CommonCommand,
};

use super::CommandOpcode;

#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
/// CNS Value
/// See: NVM Express Base Spec: Figure 274: Identify CNS Value
pub enum ControllerOrNamespace {
    Namespace {
        nsid: u32,
    },
    Controller,
    ActiveNamespaces {
        /// only active namespace ids greater this will be returned.
        ///
        /// Can be 0 to get nsid 1. Should never be `0xffff_ffff` or `0xffff_fffe`
        starting_nsid: u32,
    },
    IOCommandSet {
        controller_id: ControllerId,
    },
}

impl ControllerOrNamespace {
    pub fn cns(&self) -> u8 {
        match self {
            ControllerOrNamespace::Namespace { nsid: _ } => 0x0,
            ControllerOrNamespace::Controller => 0x1,
            ControllerOrNamespace::ActiveNamespaces { starting_nsid: _ } => 0x2,
            ControllerOrNamespace::IOCommandSet { controller_id: _ } => 0x1c,
        }
    }
}

/// Create the [CommonCommand] data structure for an identify command
///
/// See: NVM Express Base Spec: 5.17
pub fn create_identify_command(cns: ControllerOrNamespace, data_frame: PhysFrame) -> CommonCommand {
    let mut cdw0 = CDW0::zero();
    cdw0.set_opcode(CommandOpcode::Identify as u8);
    cdw0.set_prp_or_sgl(PrpOrSgl::Prp);

    let mut command = CommonCommand::default();
    command.dword0 = cdw0;
    let mut dword10: u32 = 0;
    dword10.set_bits(0..=8, cns.cns() as u32);

    match cns {
        ControllerOrNamespace::Namespace { nsid }
        | ControllerOrNamespace::ActiveNamespaces {
            starting_nsid: nsid,
        } => {
            command.namespace_ident = nsid;
        }
        ControllerOrNamespace::IOCommandSet { controller_id } => {
            dword10.set_bits(16..=31, controller_id.as_u32());
        }
        _ => {
            // NSID, CNTID and CSI not used
        }
    }
    command.dword10 = dword10;

    command.data_ptr.prp_entry_1 = data_frame.start_address();
    command
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControllerId(u16);

impl ControllerId {
    #[inline]
    fn as_u32(self) -> u32 {
        self.0 as u32
    }
}

/// Information about a Controller
///
/// See: NVM Express Base Spec: Figure 276: Identify Controller Data Structure
#[repr(C)]
#[allow(missing_docs)]
pub struct IdentifyControllerData {
    pub pci_vendor_id: u16,
    pub pci_subsystem_vendor_id: u16,
    pub serial_number: [u8; 20],
    pub model_number: [u8; 40],
    pub firmware_revison: [u8; 8],
    pub recommended_arbitration_burst: u8,
    pub ieee_oui_identifier: [u8; 3],
    pub cmic: u8,
    pub maximum_data_transfer_size: u8,
    pub controller_id: ControllerId,
    pub version: u32,
    pub rtd3_resume_latency: u32,
    pub rtd3_entry_latency: u32,
    pub optional_async_events_supported: u32,
    pub controller_attributes: u32,
    pub read_recovery_levels_supported: u16,
    reserved1: [u8; 9],
    pub controller_type: u8,
    pub fru_guid: [u8; 16],
    pub command_retry_delay_time1: u16,
    pub command_retry_delay_time2: u16,
    pub command_retry_delay_time3: u16,
    reserved2: [u8; 106],
    reserved3: [u8; 13],
    pub nvm_subsystem_report: u8,
    pub vpd_write_cycle_info: u8,
    pub management_endpoint_caps: u8,
    pub optional_admin_command_support: u16,
    // ...
    pub unimplemented1: [u8; 264],

    pub fuses: u16,
}

bitflags! {
    /// Description of an IO Command set supported by a Controller
    ///
    /// See: NVM Express Base Spec: Figure 291: I/O Command Set Vector
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[allow(missing_docs)]
    pub struct IOCommandSetVector: u64 {
        const NVM = 1;
        const KEY_VALUE = 1 << 1;
        const ZONED_NAMESPACE = 1 << 2;

        const _ = !0;
    }
}

impl IOCommandSetVector {
    pub fn valid(&self) -> bool {
        !self.is_empty()
    }
}

/// Iterator for [IOCommandSetVector]s within a [Page]
///
/// Must be constructed with the data-page for a [ControllerOrNamespace::IOCommandSet] identify
/// command.
pub struct IOCommandSetVectorIterator<'a> {
    base: VirtAddr,
    next_index: u32,
    done: bool,
    _lifetime: PhantomData<&'a ()>,
}

impl IOCommandSetVectorIterator<'_> {
    /// Creates a new [IOCommandSetVectorIterator]
    ///
    /// # Safety
    ///
    /// `vaddr` must be a valid u64 aligned reference within a
    /// page used for an identify command using the [ControllerOrNamespace::IOCommandSet]
    /// cns.
    pub unsafe fn from_vaddr(vaddr: VirtAddr) -> Self {
        let page = Page::<Size4KiB>::containing_address(vaddr);
        let base = page.start_address();
        let start_offset = vaddr - base;
        let next_index = start_offset / 8;
        let next_index: u32 = next_index.try_into().unwrap();
        Self {
            base,
            next_index,
            done: false,
            _lifetime: PhantomData,
        }
    }
}

impl<'a> Iterator for IOCommandSetVectorIterator<'a> {
    type Item = (u32, IOCommandSetVector);

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        let vec_ptr = (self.base + 8 * self.next_index as u64).as_ptr();
        let vec: IOCommandSetVector = unsafe {
            // Safety: Safe as long as iter is created with save vaddr
            *vec_ptr
        };
        if vec.valid() {
            let idx = self.next_index;
            if let Some(next) = self.next_index.checked_add(1) {
                self.next_index = next;
            } else {
                self.done = true;
            }
            Some((idx, vec))
        } else {
            self.done = true;
            None
        }
    }
}

/// Information about a Namespace
///
/// See: NVM Command Set Spec: Figure 97: Identify Namespace Data Structure
#[repr(C)]
#[allow(missing_docs)]
pub struct IdentifyNamespaceData {
    /// The size of the namespace in logical blocks
    pub namespace_size: u64,
    /// maximum number of logical blocks that can ge allocated in the namespace
    ///
    /// this is less or equal to size
    pub namespace_capacity: u64,
    /// number of allocated logical blocks
    ///
    /// this is less or equal to capacity
    pub namespace_utilization: u64,

    pub features: NamespaceFeatures,

    /// This is a 0 based value
    pub num_lba_formats: u8,

    pub formatted_lba_size: FormattedLBASize,

    pub metadata_caps: NamespaceMetadataCapabilities,

    pub e2e_data_prot_caps: u8,
    pub e2e_data_prot_type_settings: u8,
    pub multipath_io_sharing_caps: u8,
    pub reservation_caps: u8,
    pub format_progress_indicator: u8,
    pub deallocate_block_features: u8,

    pub atomic_write_normal: u16,
    pub atomic_write_power_fail: u16,
    pub atomic_write_compare_write_unit: u16,
    pub atomic_boundary_size_normal: u16,
    pub atomic_boundary_offset: u16,
    pub atomic_boundary_size_power_fail: u16,

    pub optimal_io_boundary: u16,

    /// the size of the NVM allocated to this namspace in bytes.
    ///
    /// Note: This field may not correspond to the logical block size multiplied by the Namespace
    /// Size field. Due to thin provisioning or other settings (e.g., endurance), this field may be
    /// larger or smaller than the product of the logical block size and the Namespace Size
    /// reported.
    pub nvm_capacity: u128,

    pub prefered_write_granularity: u16,
    pub prefered_write_alignment: u16,
    pub prefered_deallocate_granularity: u16,
    pub prefered_deallocate_alignment: u16,
    pub optimal_write_size: u16,

    pub maximum_single_source_range_length: u16,
    pub maximum_copy_length: u32,
    pub maximum_source_range_count: u8,

    reserved1: [u8; 10],

    pub ana_group_id: u32,
    reserved2: [u8; 3],

    pub attribues: u8,
    pub nvm_set_id: u16,
    pub endurance_group_id: u16,

    pub namespace_guid: [u8; 16],
    pub ieee_uid: [u8; 8],

    lba_formats: [LBAFormatData; 64],
}

#[allow(missing_docs)]
impl IdentifyNamespaceData {
    /// # Saftey:
    ///
    /// The virtual address is the vaddr used for a namespace identify command
    /// that finished successfully
    pub unsafe fn from_vaddr<'a>(vaddr: VirtAddr) -> Option<&'a Self> {
        const SIZE: usize = size_of::<IdentifyNamespaceData>();
        let raw_data: &[u8; SIZE] = unsafe { &*vaddr.as_ptr() };

        if raw_data == &[0; SIZE] {
            return None;
        }

        Some(unsafe { &*vaddr.as_ptr() })
    }

    pub fn active_lba_format(&self) -> LBAFormatData {
        self.get_lba_format(self.formatted_lba_size.active_format_index())
    }

    pub fn get_lba_format(&self, id: usize) -> LBAFormatData {
        self.lba_formats()[id]
    }

    pub fn lba_formats(&self) -> &[LBAFormatData] {
        &self.lba_formats[0..self.num_lba_formats as usize]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct LBAFormatData {
    /// number of metadatabytes provvided per LBA
    ///
    /// This is based on the LBA data size and shall be 0 if there is no
    /// metadata support
    pub metadata_size: u16,
    /// Indicates a supported LBA data size.
    ///
    /// The value is reported as a power of 2.
    /// A non-zero value less tahn 9 (512 bytes) is not supported.
    ///
    /// The value is 0 if the LBA format is not currently available.
    pub lba_data_size_raw: u8,

    flags: u8,

    reserved: u8,
}

impl LBAFormatData {
    /// true if the format is currently available
    pub fn is_active(&self) -> bool {
        self.lba_data_size_raw != 0
    }

    /// false if the data size of the format is invalid based on the spec
    pub fn is_valid(&self) -> bool {
        self.lba_data_size_raw != 0 && self.lba_data_size_raw >= 9
    }

    /// the size of the lba in bytes
    pub fn lba_data_size(&self) -> u64 {
        2u64.strict_pow(self.lba_data_size_raw as u32)
    }

    /// the relative preformance of the format
    pub fn relative_perf(&self) -> LBAFormatRelativePerf {
        (self.flags & 0b11)
            .try_into()
            .expect("All 2 bit variants should be valid")
    }
}

#[derive(U8Enum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LBAFormatRelativePerf {
    Best = 0,
    Better = 0b01,
    Good = 0b10,
    Degraded = 0b11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(missing_docs)]
pub struct FormattedLBASize(u8);

#[allow(missing_docs)]
impl FormattedLBASize {
    pub fn active_format_index(&self) -> usize {
        (self.0 & 0b1111) as usize
    }

    pub fn extended_logical_block(&self) -> bool {
        self.0.get_bit(4)
    }
}

bitflags! {
    /// Bitflag describing the supported namespace features
    ///
    /// See: NVM Command Set Spec: Figure 97: Identify Namespace Data Structure - NSFEAT
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    #[allow(missing_docs)]
    pub struct NamespaceFeatures: u8 {
        const THIN_PROVISIONING = 1;
        const NSABP = 1 << 1;
        const DAE = 1 << 2;
        const UIDREUSE = 1 << 3;
        const OPTPERF = 1 << 4;

        const _ = 0b11100000;
    }

    /// Bitflag describing the metadata capabilities of a namspace
    ///
    /// See: NVM Command Set Spec: Figure 97: Identify Namespace Data Structure - MC
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    #[allow(missing_docs)]
    pub struct NamespaceMetadataCapabilities: u8 {
        const EXTENDED_DATA_LBA = 1;
        const METADATA_POINTER = 1 << 1;

        // .. unimplemented

        const _ = !0;
    }
}
