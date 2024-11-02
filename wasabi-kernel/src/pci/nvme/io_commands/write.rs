use bit_field::BitField;
use x86_64::structures::paging::PhysFrame;

use crate::pci::nvme::{
    generic_command::{PrpOrSgl, CDW0},
    CommonCommand,
};

use super::{CommandOpcode, LBA};

/// Create the [CommonCommand] data structure for a write command
///
/// See: NVM Command Spec: 3.2.6
#[allow(unused)]
pub fn create_write_command(frame: PhysFrame, slba: LBA, block_count: u16) -> CommonCommand {
    let mut cdw0 = CDW0::zero();
    cdw0.set_opcode(CommandOpcode::Read as u8);
    cdw0.set_prp_or_sgl(PrpOrSgl::Prp);

    let mut command = CommonCommand::default();
    command.dword0 = cdw0;

    command.namespace_ident = 1;
    command.data_ptr.prp_entry_1 = frame.start_address();

    // ignored because we don't use protection
    command.dword2 = 0;
    command.dword3 = 0;

    let slba = slba.value();
    command.dword10 = (slba & 0xffff_ffff) as u32;
    command.dword11 = (slba >> 32) as u32;

    let mut dword12 = 0;
    // dword12.set_bit(31, true); // limit retry

    // set block count as 0 based value
    assert!(block_count > 0);
    dword12.set_bits(0..=15, (block_count - 1) as u32);

    command.dword12 = dword12;

    // bit 6 is used for sequential writes
    // bit 0..=5 is used for access frequency and latency hints
    command.dword13 = 0;

    command
}
