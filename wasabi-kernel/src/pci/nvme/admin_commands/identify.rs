use core::marker::PhantomData;

use bit_field::BitField;
use bitflags::bitflags;
use x86_64::{
    structures::paging::{Page, PhysFrame, Size4KiB},
    VirtAddr,
};

use crate::pci::nvme::{CommonCommand, PrpOrSgl, CDW0};

use super::CommandOpcode;

#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
/// CNS Value
/// See: NVM Express Base Spec: Figure 274: Identify CNS Value
pub enum IdentifyNamespaceIdent {
    Namespace { nsid: u32 },
    Controller,
    IOCommandSet { controller_id: ControllerId },
}

impl IdentifyNamespaceIdent {
    pub fn cns(&self) -> u8 {
        match self {
            IdentifyNamespaceIdent::Namespace { nsid: _ } => 0x0,
            IdentifyNamespaceIdent::Controller => 0x1,
            IdentifyNamespaceIdent::IOCommandSet { controller_id: _ } => 0x1c,
        }
    }
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
    cdw0.set_prp_or_sgl(PrpOrSgl::Prp);

    let mut command = CommonCommand::default();
    command.dword0 = cdw0;
    let mut dword10: u32 = 0;
    dword10.set_bits(0..=8, cns.cns() as u32);

    match cns {
        IdentifyNamespaceIdent::Namespace { nsid } => {
            command.namespace_ident = nsid;
        }
        IdentifyNamespaceIdent::IOCommandSet { controller_id } => {
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
    controller_type: u8,
    fru_guid: [u8; 16],
    command_retry_delay_time1: u16,
    command_retry_delay_time2: u16,
    command_retry_delay_time3: u16,
    reserved2: [u8; 106],
    reserved3: [u8; 13],
    nvm_subsystem_report: u8,
    vpd_write_cycle_info: u8,
    management_endpoint_caps: u8,
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
/// Must be constructed with the data-page for a [IdentifyNamespaceIdent::IOCommandSet] identify
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
    /// page used for an identify command using the [IdentifyNamespaceIdent::IOCommandSet]
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
