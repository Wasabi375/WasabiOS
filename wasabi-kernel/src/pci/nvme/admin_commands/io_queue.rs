use bit_field::BitField;
use bitflags::bitflags;
use shared_derive::U8Enum;
use x86_64::structures::paging::frame::PhysFrameRangeInclusive;

use crate::pci::nvme::{
    admin_commands::CommandOpcode, CommonCommand, PrpOrSgl, QueueIdentifier, CDW0,
    COMPLETION_COMMAND_ENTRY_SIZE, SUBMISSION_COMMAND_ENTRY_SIZE,
};

#[allow(unused_imports)]
use log::trace;

pub fn create_io_completion_queue(
    queue_ident: QueueIdentifier,
    queue_size: u16,
    frames: PhysFrameRangeInclusive,
) -> CommonCommand {
    trace!(
        "create io completion queue command, ident: {:?}, size: {}",
        queue_ident,
        queue_size
    );

    let mut dword0 = CDW0::zero();
    dword0.set_opcode(CommandOpcode::CreateIOCompletionQueue as u8);
    dword0.set_prp_or_sgl(PrpOrSgl::Prp);

    let mut command = CommonCommand::default();

    command.dword0 = dword0;

    assert!((queue_size as u64) * COMPLETION_COMMAND_ENTRY_SIZE <= frames.size());
    command.data_ptr.prp_entry_1 = frames.start.start_address();

    let mut dword10: u32 = 0;
    dword10.set_bits(0..=15, queue_ident.as_u16() as u32);

    // Convert into 0 based value
    assert!(queue_size > 0);
    let queue_size = queue_size - 1;

    dword10.set_bits(16..=31, queue_size as u32);
    command.dword10 = dword10;

    let mut dword11: u32 = 0;
    // TODO queue interrupt vector bits 16..=31
    dword11.set_bit(1, false); // disable interrupts
    dword11.set_bit(0, true); // memory is phys contiguous
    command.dword11 = dword11;

    command
}

pub fn create_io_submission_queue(
    queue_ident: QueueIdentifier,
    queue_size: u16,
    frames: PhysFrameRangeInclusive,
) -> CommonCommand {
    trace!(
        "create io submission queue command, ident: {:?}, size: {}",
        queue_ident,
        queue_size
    );

    let mut dword0 = CDW0::zero();
    dword0.set_opcode(CommandOpcode::CreateIOSubmissionQueue as u8);
    dword0.set_prp_or_sgl(PrpOrSgl::Prp);

    let mut command = CommonCommand::default();

    command.dword0 = dword0;

    assert!((queue_size as u64) * SUBMISSION_COMMAND_ENTRY_SIZE <= frames.size());
    command.data_ptr.prp_entry_1 = frames.start.start_address();

    let mut dword10: u32 = 0;
    dword10.set_bits(0..=15, queue_ident.as_u16() as u32);

    // Convert into 0 based value
    assert!(queue_size > 0);
    let queue_size = queue_size - 1;

    dword10.set_bits(16..=31, queue_size as u32);
    command.dword10 = dword10;

    let mut dword11: u32 = 0;
    dword11.set_bits(16..=31, queue_ident.0 as u32); // completion queue, we use a 1:1 mapping
    dword11.set_bits(1..=2, 0b10); // queue priority, ignored, but we set a default of medium anyways
    dword11.set_bit(0, true); // memory is phys contiguous
    command.dword11 = dword11;

    command
}

pub fn delete_io_completion_queue() -> CommonCommand {
    todo!()
}

pub fn delete_io_submission_queue() -> CommonCommand {
    todo!()
}

bitflags! {
    // TODO Is this an enum or a bitmap?
    #[allow(missing_docs)]
    #[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy)]
    pub struct CompletionQueueCreationStatus: u8 {
        const InvalidIdentifier = 0x1;
        const InvalidSize = 0x2;
        const InvalidInterruptVector = 0x8;
    }
}

#[allow(missing_docs)]
#[repr(u8)]
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone, Copy, U8Enum)]
pub enum SubmissionQueueCreationStatus {
    InvalidCompletionQueue = 0x0,
    InvalidIdentifier = 0x1,
    InvalidSize = 0x2,
}
