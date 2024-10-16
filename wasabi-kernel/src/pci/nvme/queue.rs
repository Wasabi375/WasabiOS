//! Data structures and functions for using NVME Command Queues.
//!
//! The specification documents can be found at https://nvmexpress.org/specifications/
//! specifically: NVM Express Base Specification

use core::{hint::spin_loop, ops::Add};

use alloc::{collections::VecDeque, vec::Vec};
use derive_where::derive_where;
use shared::{
    math::WrappingValue,
    sync::lockcell::{LockCell, LockCellGuard, LockCellInternal},
};
use thiserror::Error;
use volatile::{access::WriteOnly, Volatile};
use x86_64::{
    structures::paging::{Mapper, Page, PageSize, PageTableFlags, PhysFrame, Size4KiB},
    PhysAddr, VirtAddr,
};

use crate::{
    map_page,
    mem::{
        frame_allocator::WasabiFrameAllocator, page_allocator::PageAllocator,
        page_table::KERNEL_PAGE_TABLE, MemError, VirtAddrExt,
    },
};

use super::{
    CommandIdentifier, CommonCommand, CommonCompletionEntry, NVMEControllerError,
    COMPLETION_COMMAND_ENTRY_SIZE, SUBMISSION_COMMAND_ENTRY_SIZE,
};

#[allow(unused_imports)]
use crate::{todo_error, todo_warn};
#[allow(unused_imports)]
use log::{debug, info, trace, warn};

#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord)]
pub struct QueueIdentifier(pub(super) u16);

impl QueueIdentifier {
    pub fn as_u16(self) -> u16 {
        self.0
    }

    pub fn checked_add(self, rhs: u16) -> Option<Self> {
        self.0.checked_add(rhs).map(|v| Self(v))
    }
}

impl Add<u16> for QueueIdentifier {
    type Output = QueueIdentifier;

    fn add(self, rhs: u16) -> Self::Output {
        QueueIdentifier(self.0 + rhs)
    }
}

/// A data structure giving access to an NVME command queue.
///
/// This supports both admin and io command sets, although
/// a single instance will only support either and not both.
///
/// # Safety
///
/// Before this is constructed a corresponding queue has to be allocated
/// on the NVME controller. This should normally be done using
/// [NVMEController::allocate_io_queues].
/// The [NVMEController] must ensure that the queue on the nvme device is disabled
/// before this is dropped.
#[derive_where(Debug)]
pub struct CommandQueue {
    id: QueueIdentifier,

    pub(super) submission_queue_size: u16,
    pub(super) submission_queue_paddr: PhysAddr,
    pub(super) submission_queue_vaddr: VirtAddr,

    /// dorbell that is written to to inform the controller that
    /// new command entries have been submited
    #[derive_where(skip)]
    pub(super) submission_queue_tail_doorbell: Volatile<&'static mut u32, WriteOnly>,
    /// The last submission entry index we notified the controller about.
    ///
    /// This diferes from [CommandQueue::submission_queue_tail_local]
    /// in that the local version is updated when [CommandQueue::submit] is
    /// called - when we write the command entry.
    /// This is updated when [CommandQueue::flush] is called - when we inform
    /// the controller about the submitted command entries via the doorbell.
    ///
    /// See: NVMe Base Spec: 3.3.1.5: Full Queue
    ///
    /// [CommandQueue::submission_queue_tail_doorbell] is writeonly
    /// so we keep a copy of the value here
    submission_queue_tail: u16,
    /// the last entry read by the controller.
    ///
    /// this value is set by the controller in each completion entry.
    submission_queue_head: u16,

    /// Indicates the index of the next "solt" to write a command entry
    /// in order to submit it to the controller. The slot is only free
    /// if the head is sufficiently ahead of the tail.
    submission_queue_tail_local: WrappingValue<u16>,

    pub(super) completion_queue_size: u16,
    pub(super) completion_queue_paddr: PhysAddr,
    pub(super) completion_queue_vaddr: VirtAddr,

    /// dorbell that is written to to inform the controller that
    /// completion entries have been read, freeing the slots
    /// for the controller to fill with new completion entries
    #[derive_where(skip)]
    pub(super) completion_queue_head_doorbell: Volatile<&'static mut u32, WriteOnly>,

    /// the next entry to read from this completion queue.
    ///
    /// [CommandQueue::completion_queue_head_doorbell] is writeonly
    /// so we keep a copy of the value here.
    completion_queue_head: WrappingValue<u16>,

    /// The expected phase of the next completion entry.
    ///
    /// This starts out at `true` and switches every time the completion queue
    /// wraps around to the `0th` index
    completion_expected_phase: bool,

    /// Completion entries read from the controller
    ///
    /// We store completions that are polled from the controller here,
    /// so that they can be accessed by users of the [CommandQueue] without
    /// blocking incomming completions
    // TODO do I want some sort of max size restriction for this? How would that work?
    #[derive_where(skip)]
    completions: VecDeque<CommonCompletionEntry>,

    /// A unique id used to identify a command
    ///
    /// This value is both present on the submission and completion and needs to be unique
    /// within a queue.
    ///
    /// TODO the value of 0xffff should never be used
    next_command_identifier: u16,
}

impl CommandQueue {
    /// calculates the submission and completion doorbells for a [CommandQueue]
    ///
    /// # Arguments:
    /// * `doorbell_base`: The [VirtAddr] for the first doorbell. This should
    ///         be in the properties at offset `0x1000`
    /// * `stride`: [properties::Capabilites::doorbell_stride] for this controller
    /// * `queue_index`: The index of the queue, starting at 0 for the admin queu
    ///         and iterating through the I/O queues
    ///
    /// # Saftey:
    /// `doorbell_base` must be valid to write to for all queue doorbells up to
    /// `queue_index`.
    /// Caller must also ensure that no alias exists for the returned unique references
    pub(super) unsafe fn get_doorbells(
        doorbell_base: VirtAddr,
        stride: u64,
        queue_index: QueueIdentifier,
    ) -> (
        Volatile<&'static mut u32, WriteOnly>,
        Volatile<&'static mut u32, WriteOnly>,
    ) {
        assert!(stride >= 4);

        let submission = doorbell_base + (queue_index.0 as u64 * stride * 2);
        let completion = submission + stride;

        trace!("doorbells: sub {:p}, comp {:p}", submission, completion);

        unsafe {
            // Safety: see outer function
            (
                submission.as_volatile_mut().write_only(),
                completion.as_volatile_mut().write_only(),
            )
        }
    }

    /// Allocates a new command queue.
    ///
    /// This queue is only useable after the nvme controller has also created
    /// the completion and submission queue on the nvme device.
    ///
    /// # Saftey:
    /// * `doorbell_base` must be valid to write to for all queue doorbells up to `queue_id` while
    ///     this is alive.
    /// * Must not be called with the same `queue_id` as long as the resulting [CommandQueue] is
    ///     alive.
    /// * Caller ensures that all needed allocations on the [NVMEController] outlive this.
    /// * Caller must also ensure that the queue on the controller is disabled before dropping this.
    pub(super) unsafe fn allocate<L1, L2>(
        queue_id: QueueIdentifier,
        submission_queue_size: u16,
        completion_queue_size: u16,
        doorbell_base: VirtAddr,
        queue_doorbell_stride: u64,
        frame_allocator: &mut LockCellGuard<WasabiFrameAllocator<'static, Size4KiB>, L1>,
        page_allocator: &mut LockCellGuard<PageAllocator, L2>,
    ) -> Result<Self, NVMEControllerError>
    where
        L1: LockCellInternal<WasabiFrameAllocator<'static, Size4KiB>>,
        L2: LockCellInternal<PageAllocator>,
    {
        if submission_queue_size < 2 {
            return Err(NVMEControllerError::InvalidQueueSize(submission_queue_size));
        }
        if completion_queue_size < 2 {
            return Err(NVMEControllerError::InvalidQueueSize(completion_queue_size));
        }

        let (sub_tail_doorbell, comp_head_doorbell) = unsafe {
            // Safety: see our safety
            CommandQueue::get_doorbells(doorbell_base, queue_doorbell_stride, queue_id)
        };

        let sub_memory_size = submission_queue_size as u64 * SUBMISSION_COMMAND_ENTRY_SIZE;

        if sub_memory_size > Size4KiB::SIZE {
            todo_error!("command queue larger than 1 page");
            return Err(NVMEControllerError::InvalidQueueSize(submission_queue_size));
        }

        let comp_memory_size = completion_queue_size as u64 * COMPLETION_COMMAND_ENTRY_SIZE;
        if comp_memory_size > Size4KiB::SIZE {
            todo_error!("command queue larger than 1 page");
            return Err(NVMEControllerError::InvalidQueueSize(completion_queue_size));
        }

        let sub_frame = frame_allocator.alloc().ok_or(MemError::OutOfMemory)?;
        let sub_page = page_allocator.allocate_page_4k()?;
        let submission_queue_paddr = sub_frame.start_address();
        let submission_queue_vaddr = sub_page.start_address();

        let comp_frame = frame_allocator.alloc().ok_or(MemError::OutOfMemory)?;
        let comp_page = page_allocator.allocate_page_4k()?;
        let completion_queue_paddr = comp_frame.start_address();
        let completion_queue_vaddr = comp_page.start_address();

        let queue_pt_flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_CACHE
            | PageTableFlags::NO_EXECUTE;
        unsafe {
            // Safety: we just allocated page and frame
            map_page!(
                sub_page,
                Size4KiB,
                queue_pt_flags,
                sub_frame,
                frame_allocator.as_mut()
            )?;

            // Safety: we just mapped this region of memory
            submission_queue_vaddr.zero_memory(sub_memory_size as usize);

            // Safety: we just allocated page and frame
            map_page!(
                comp_page,
                Size4KiB,
                queue_pt_flags,
                comp_frame,
                frame_allocator.as_mut()
            )?;

            // Safety: we just mapped this region of memory
            completion_queue_vaddr.zero_memory(comp_memory_size as usize);
        }

        Ok(Self {
            id: queue_id,
            submission_queue_size,
            submission_queue_paddr,
            submission_queue_vaddr,
            completion_queue_size,
            completion_queue_paddr,
            completion_queue_vaddr,
            submission_queue_tail_doorbell: sub_tail_doorbell,
            submission_queue_tail: 0,
            submission_queue_tail_local: WrappingValue::zero(submission_queue_size),
            submission_queue_head: 0,
            completion_queue_head_doorbell: comp_head_doorbell,
            completion_queue_head: WrappingValue::zero(completion_queue_size),
            completion_expected_phase: true,
            completions: VecDeque::new(),
            next_command_identifier: 0,
        })
    }

    /// The [QueueIdentifier] for this [CommandQueue]
    pub fn id(&self) -> QueueIdentifier {
        self.id
    }

    /// Submits a new [CommonCommand] to the [CommandQueue].
    ///
    /// Commands are only executed by the nvme device after [Self::flush] is called.
    /// if [Self::cancel_submissions] is called instead all submited commands since
    /// the last call to [Self::flush] will be ignored.
    pub fn submit(
        &mut self,
        mut command: CommonCommand,
    ) -> Result<CommandIdentifier, NVMEControllerError> {
        if self.is_full_for_submission() {
            let new_completions = self.poll_completions().some_new_entries;
            if !new_completions || self.is_full_for_submission() {
                return Err(NVMEControllerError::QueueFull);
            }
        }

        let identifier = CommandIdentifier(self.next_command_identifier);
        // TODO identifier should never be 0xffff as some functions use 0xffff in the completion
        // to refer to a general error that is not associated with any command
        self.next_command_identifier = self.next_command_identifier.wrapping_add(1);

        command.dword0.set_command_identifier(identifier);

        trace!("submit command({identifier:?}) to queue");

        let entry_slot_index = self.submission_queue_tail_local.value();
        self.submission_queue_tail_local += 1;

        let slot_vaddr = (self.submission_queue_vaddr
            + (SUBMISSION_COMMAND_ENTRY_SIZE * entry_slot_index as u64))
            .as_mut_ptr::<CommonCommand>();

        assert!(slot_vaddr.is_aligned());
        unsafe {
            // Safety: submission queue is properly mapped to allow write access
            slot_vaddr.write_volatile(command);
        }

        Ok(identifier)
    }

    /// Cancells all submissions since the last call to [Self::flush].
    pub fn cancel_submissions(&mut self) {
        debug!("Cancel submissions for queue {:?}", self.id);
        self.submission_queue_tail_local =
            WrappingValue::new(self.submission_queue_tail, self.submission_queue_size);
    }

    /// Notify the controller about any pending submission command entries
    pub fn flush(&mut self) {
        if !self.has_submissions_pending_flush() {
            warn!("no submissions pending");
            return;
        }

        trace!(
            "flush command queue by writting {:#x} to doorbell",
            self.submission_queue_tail_local.value()
        );

        self.submission_queue_tail_doorbell
            .write(self.submission_queue_tail_local.value() as u32);
        self.submission_queue_tail = self.submission_queue_tail_local.value();
    }

    /// returns `true` if this queue is full for submissions
    ///
    /// Check completions to advance the last read submission entry
    /// of the controller
    ///
    /// See: NVMe Base Spec: 3.3.1.5: Full Queue
    pub fn is_full_for_submission(&self) -> bool {
        self.submission_queue_head == (self.submission_queue_tail_local + 1).value()
    }

    /// returns `true` if there are no submission command entries pending
    ///
    /// See: NVMe Base Spec: 3.3.1.4: Empty Queue
    pub fn is_submissions_empty(&self) -> bool {
        self.submission_queue_head == self.submission_queue_tail_local.value()
    }

    /// returns `true` as long as there are submission in the queue
    /// that the controller has not been notified about.
    ///
    /// Use [CommandQueue::flush] to notify the controller about new
    /// entries and clear this flag.
    pub fn has_submissions_pending_flush(&self) -> bool {
        self.submission_queue_tail != self.submission_queue_tail_local.value()
    }

    /// Poll the controller for new completion entries.
    ///
    /// # Returns
    ///
    /// * Ok(true): if at least 1 entry was found and all entries were successfully
    ///         added to the completions list.
    /// * Ok(false): if no entries were found.
    /// * Err((true, CompletionPollError)):  at least 1 entry was found and successfully
    ///         added to the completions list, but 1 additional entry was found,
    ///         that could not be added to the completions list
    /// * Err((false, CompletionPollError)): No entires were found and added to the completions
    ///         list
    pub fn poll_completions(&mut self) -> PollCompletionsResult {
        let mut any_found = false;
        let mut error = None;
        loop {
            match self.poll_single_completion() {
                Ok(true) => {
                    any_found = true;
                    continue;
                }
                Ok(false) => break,
                Err(reason) => {
                    error = Some(reason);
                    break;
                }
            }
        }
        if any_found {
            // we found at least 1 entry, so inform the controller that about the
            // read entries
            self.completion_queue_head_doorbell
                .write(self.completion_queue_head.value() as u32);
        }

        PollCompletionsResult {
            some_new_entries: any_found,
            error_on_any_entry: error,
        }
    }

    /// poll the controller for a single new completion entry.
    ///
    /// this will upate the `submission_queue_head` and `completion_queu_head`,
    /// but not trigger the completion queue head doorbell.
    /// The caller is responsible for triggering the completion queue head
    /// doorbell instead by setting it to `completion_queue_head`.
    /// This will also flip `completion_expected_phase` when necessary.
    ///
    /// # Returns
    ///  
    /// ## `Ok(true)`
    ///
    /// if a new completion entry was found and successfully added to
    /// `completions`. In this case `submission_queue_head` and `completion_queue_head`
    /// are advanced.
    ///
    /// ## `Ok(false)`
    ///
    /// no new completion entry was found, this does not modify any state.
    ///
    /// ## `Err(CompletionPollError::IdentifierStillInUse)`
    ///
    /// a new completion entry was found, but the `completions` list still
    /// contains an entry with the same identifier.
    /// Only `submission_queue_head` is advanced. `completion_queue_head`
    /// and `completion_expected_phase` are left unchanged.
    fn poll_single_completion(&mut self) -> Result<bool, PollCompletionError> {
        let slot_vaddr = self.completion_queue_vaddr
            + (COMPLETION_COMMAND_ENTRY_SIZE * self.completion_queue_head.value() as u64);
        let possible_completion = unsafe {
            // Safety: completion queue is properly mapped for read access
            slot_vaddr.as_ptr::<CommonCompletionEntry>().read_volatile()
        };

        if possible_completion.status_and_phase.phase() != self.completion_expected_phase {
            // phase did not match, therefor this is the old completion entry
            return Ok(false);
        }

        // NOTE: It should be fine to keep using possible_completion after checking the phase.
        // For some reason qemu returns only a partial completion entry on first read.
        // It seems there is a race event in qemu or something.
        // We drop and read the completion again to ensure we don't encounter that race
        drop(possible_completion);

        let completion = unsafe {
            // Safety: completion queue is properly mapped for read access
            slot_vaddr.as_ptr::<CommonCompletionEntry>().read_volatile()
        };
        assert_eq!(
            completion.status_and_phase.phase(),
            self.completion_expected_phase
        );
        assert_eq!(
            self.id, completion.submission_queue_ident,
            "Submission queue mismatch on polled completion entry"
        );

        // it is fine to update the submission head, even if we can not yet store this completion
        // because this only indicates that the controller has read the submission
        // command, not that it is fully handled
        //
        // the controller ensures this is wrapped around to 0 when neccessary.
        self.submission_queue_head = completion.submission_queue_head;

        // binary search returns the index to insert at in the Err
        // if Ok the key is already in use, so we have to error
        //
        // we need to check this before incrementing the head index.
        // Otherwise this completion can never be read
        let Err(insert_at) = self
            .completions
            .binary_search_by_key(&completion.command_ident, |c| c.command_ident)
        else {
            return Err(PollCompletionError::IdentifierStillInUse(
                completion.command_ident,
            ));
        };

        trace!("Polled Completion entry({:?})", completion.command_ident);

        // TODO document why we increment here and not earlier
        self.completion_queue_head += 1;
        if self.completion_queue_head.value() == 0 {
            trace!("completion queue wrapping");
            self.completion_expected_phase = !self.completion_expected_phase;
        }

        self.completions.insert(insert_at, completion);

        Ok(true)
    }

    /// Iterate over all polled [CommonCompletionEntries](CommonCompletionEntry).
    ///
    /// New entries are only visible after [Self::poll_completions] was executed.
    pub fn iter_completions(&self) -> impl Iterator<Item = &CommonCompletionEntry> {
        self.completions.iter()
    }

    /// Iterate and drain all polled [CommonCompletionEntries](CommonCompletionEntry).
    ///
    /// New entries are only visible after [Self::poll_completions] was executed.
    pub fn drain_completions(&mut self) -> impl Iterator<Item = CommonCompletionEntry> + '_ {
        self.completions.drain(..)
    }

    /// clears all outstanding completions.
    ///
    /// This can lead to bugs if someone is still "waiting" for an completion entry that is dropped
    /// by this function call.
    pub(super) fn clear_completions(&mut self) {
        loop {
            let _ = self.drain_completions();
            let poll_result = self.poll_completions();
            assert!(
                poll_result.error_on_any_entry.is_none(),
                "We just cleard the completions, therefor there cant be an conflicting command ident"
            );
            if !poll_result.some_new_entries {
                break;
            }
        }
    }

    /// Wait until a [CommonCompletionEntry] for a specific command exists.
    pub fn wait_for(
        &mut self,
        ident: CommandIdentifier,
    ) -> Result<CommonCompletionEntry, PollCompletionError> {
        trace!("Waiting for {ident:?}");
        let get_if_exists = |completions: &mut VecDeque<CommonCompletionEntry>| {
            if let Ok(index) = completions.binary_search_by_key(&ident, |c| c.command_ident) {
                return Some(
                    completions
                        .remove(index)
                        .expect("index should be valid, we just checked for it"),
                );
            }
            return None;
        };
        if let Some(entry) = get_if_exists(&mut self.completions) {
            return Ok(entry);
        }

        loop {
            let poll_result = self.poll_completions();
            if poll_result.some_new_entries {
                if let Some(entry) = get_if_exists(&mut self.completions) {
                    return Ok(entry);
                }
                // entry not in the new completions, cointinue waiting
                spin_loop()
            } else {
                if let Some(err) = poll_result.error_on_any_entry {
                    return Err(err);
                }
                // no new entries, continue waiting
                spin_loop();
            }
        }
    }

    #[inline]
    pub fn submit_all<I: IntoIterator<Item = CommonCommand>>(
        &mut self,
        commands: I,
    ) -> Result<Vec<CommandIdentifier>, NVMEControllerError> {
        let commands = commands.into_iter();
        let mut idents = Vec::with_capacity(commands.size_hint().0);

        for command in commands {
            idents.push(self.submit(command)?);
        }

        Ok(idents)
    }

    #[inline]
    pub fn wait_for_all<I: IntoIterator<Item = CommandIdentifier>>(
        &mut self,
        idents: I,
    ) -> Result<Vec<CommonCompletionEntry>, (Vec<CommonCompletionEntry>, PollCompletionError)> {
        let idents = idents.into_iter();
        let mut completed = Vec::with_capacity(idents.size_hint().0);

        for ident in idents {
            match self.wait_for(ident) {
                Ok(entry) => completed.push(entry),
                Err(err) => return Err((completed, err)),
            }
        }

        Ok(completed)
    }
}

impl Drop for CommandQueue {
    fn drop(&mut self) {
        trace!("dropping command queue: {:?}", self.id);
        let sub_memory_size = self.submission_queue_size as u64 * SUBMISSION_COMMAND_ENTRY_SIZE;
        assert!(sub_memory_size <= Size4KiB::SIZE);
        let sub_page = Page::<Size4KiB>::from_start_address(self.submission_queue_vaddr)
            .expect("submission_queue_vaddr should be a page start");
        let sub_frame = PhysFrame::<Size4KiB>::from_start_address(self.submission_queue_paddr)
            .expect("submission_queue_paddr should be a frame start");

        let comp_memory_size = self.completion_queue_size as u64 * COMPLETION_COMMAND_ENTRY_SIZE;
        assert!(comp_memory_size <= Size4KiB::SIZE);
        let comp_page = Page::<Size4KiB>::from_start_address(self.completion_queue_vaddr)
            .expect("completion_queue_vaddr should be a page start");
        let comp_frame = PhysFrame::<Size4KiB>::from_start_address(self.completion_queue_paddr)
            .expect("completion_queue_paddr should be a frame start");

        let mut frame_allocator = WasabiFrameAllocator::<Size4KiB>::get_for_kernel().lock();
        let mut page_allocator = PageAllocator::get_kernel_allocator().lock();
        let mut page_table = KERNEL_PAGE_TABLE.lock();

        let (_, _, flush) = page_table
            .unmap(sub_page)
            .expect("failed to unmap submission queue page");
        flush.flush();
        let (_, _, flush) = page_table
            .unmap(comp_page)
            .expect("failed to unmap submission queue page");
        flush.flush();

        unsafe {
            // # Safety:
            //
            // frames are no longer used, because we hold the only references
            // directly into the queue.
            //
            // The queue won't write to the comp_frame, because of the safety guarantees of
            // [Self::allocate].
            frame_allocator.free(sub_frame);
            frame_allocator.free(comp_frame);

            // Safety: pages are no longer used, because we hold the only references
            // directly into the queue
            page_allocator.free_page(sub_page);
            page_allocator.free_page(comp_page);
        }
    }
}

/// The return type used by [CommandQueue::poll_completions].
pub struct PollCompletionsResult {
    /// If `true` there is at least 1 new completion entry that was found.
    pub some_new_entries: bool,
    /// If `Some` then at least polling failed for at least 1 entry, meaning
    /// that an entry was found, but it is not yet possible to store it in the
    /// completions list (see [CompletionPollError]).
    /// It is possible that other entries were polled successfully.
    pub error_on_any_entry: Option<PollCompletionError>,
}

#[allow(missing_docs)]
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum PollCompletionError {
    #[error("Failed to poll command, because the identifier {0:#x} is still active!")]
    IdentifierStillInUse(CommandIdentifier),
}
