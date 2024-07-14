//! NVME pci device
//!
//! The specification documents can be found at https://nvmexpress.org/specifications/

#![allow(missing_docs)] // TODO temp

pub mod admin_commands;
pub mod properties;

use alloc::collections::VecDeque;
use core::{hint::spin_loop, mem::size_of};
use log::error;

use self::properties::{
    AdminQueueAttributes, Capabilites, ControllerConfiguration, ControllerStatus,
};
use super::{Class, PCIAccess, StorageSubclass};
use crate::{
    free_frame, free_page, map_frame, map_page,
    mem::{
        frame_allocator::WasabiFrameAllocator, page_allocator::PageAllocator,
        page_table::PageTableMapError, MemError, VirtAddrExt,
    },
    pci::{
        nvme::{
            admin_commands::{
                ControllerId, IOCommandSetVector, IOCommandSetVectorIterator,
                IdentifyControllerData,
            },
            properties::ArbitrationMechanism,
        },
        CommonRegisterOffset, Device, RegisterAddress, PCI_ACCESS,
    },
    todo_error, unmap_page,
    utils::log_hex_dump,
};

use bit_field::BitField;
use derive_where::derive_where;
use shared::sync::lockcell::LockCell;
use shared_derive::U8Enum;
use static_assertions::const_assert_eq;
use thiserror::Error;
use volatile::{access::WriteOnly, Volatile};
use x86_64::{
    structures::paging::{Page, PageSize, PageTableFlags, PhysFrame, Size4KiB},
    PhysAddr, VirtAddr,
};

#[allow(unused_imports)]
use crate::todo_warn;
#[allow(unused_imports)]
use log::{debug, info, trace, warn};

/// The size of all submission command entries
const SUBMISSION_COMMAND_ENTRY_SIZE: u64 = 64;
/// The size of all completion command entries
const COMPLETION_COMMAND_ENTRY_SIZE: u64 = 16;

#[allow(missing_docs)]
#[derive(Debug, PartialEq, Error)]
pub enum NVMEControllerError {
    #[error("Memory error: {0:?}")]
    Mem(#[from] MemError),
    #[error("PCI device class needs to be Storage with subclass NVM")]
    InvalidPciDevice,
    #[error("Queue size of {0:#x} is not valid for this queue!")]
    InvalidQueueSize(u16),
    #[error("the device is not supported: {0}")]
    DeviceNotSupported(&'static str),
    #[error("NVMe command failed: {0}")]
    CommandFailed(#[from] CommandStatusCode),
    #[error("Command queue is full")]
    QueueFull,
}

impl From<PageTableMapError> for NVMEControllerError {
    fn from(value: PageTableMapError) -> Self {
        let mem_err = MemError::from(value);
        mem_err.into()
    }
}

/// Provides communication with an NVME Storage Controller
#[derive(Debug)]
#[allow(unused)] // TODO temp
pub struct NVMEController {
    pci_dev: Device,
    controller_base_paddr: PhysAddr,
    controller_page: Page<Size4KiB>,
    controller_base_vaddr: VirtAddr,
    doorbell_page: Page<Size4KiB>,
    doorbell_base_vaddr: VirtAddr,
    admin_queue: CommandQueue,

    controller_id: Option<ControllerId>,
    io_command_sets: IOCommandSetVector,

    max_number_completions_queues: u16,
    max_number_submission_queues: u16,
}

impl NVMEController {
    /// Initializes a NVMe PCI controller
    ///
    /// See: NVM Express Base Specification: 3.5.1
    ///         Memory-based Transport Controller Initialization
    ///
    /// # Arguments:
    ///  * `io_queue_count_request`: The maximum number of queues to request. This does not create the
    ///         queues, only allocates them on the controller.
    ///         The controller can decide to allocate a different amount than requested.
    ///
    /// # Safety:
    ///
    /// This must only be called once on an NVMe [Device] until the controller is properly
    /// freed (TODO not implemented(disable controller and unmap memory)).
    // TODO proper error type
    pub unsafe fn initialize(
        pci: &mut PCIAccess,
        pci_dev: Device,
        io_queue_count_request: u16,
    ) -> Result<Self, NVMEControllerError> {
        if !matches!(
            pci_dev.class,
            Class::Storage(StorageSubclass::NonVolatileMemory)
        ) {
            return Err(NVMEControllerError::InvalidPciDevice);
        }

        if pci_dev.functions > 1 {
            todo_warn!("support multi function nvme controllers not implemented!");
        }

        let properties_base_paddr = get_controller_properties_address(pci, pci_dev, 0);

        let properties_page_table_falgs: PageTableFlags = PageTableFlags::WRITABLE
            | PageTableFlags::PRESENT
            | PageTableFlags::NO_CACHE
            | PageTableFlags::NO_EXECUTE;

        let properties_frame = PhysFrame::containing_address(properties_base_paddr);
        let properties_frame_offset = properties_base_paddr - properties_frame.start_address();
        assert_eq!(
            properties_frame_offset, 0,
            "Properties that aren't page alligned are not supported"
        );
        let properties_page = unsafe {
            // Safety: only called once for frame, assuming function safety
            map_frame!(Size4KiB, properties_page_table_falgs, properties_frame).unwrap()
        };
        let properties_base_vaddr = properties_page.start_address() + properties_frame_offset;

        trace!(
            "nvme controller properties at phys {:p} mapped to {:p}",
            properties_base_paddr,
            properties_base_vaddr
        );

        /// See NVME over PCIe Transport Spec: Figure 4: PCI Express Specific Property
        /// Definitions
        const DOORBELL_PHYS_OFFSET: u64 = 0x1000;
        let doorbell_frame =
            PhysFrame::from_start_address(properties_base_paddr + DOORBELL_PHYS_OFFSET).unwrap();
        let doorbell_page = unsafe {
            // Safety: only called once for frame, assuming function safety
            map_frame!(Size4KiB, properties_page_table_falgs, doorbell_frame)?
        };
        let doorbell_base: VirtAddr = doorbell_page.start_address();
        trace!(
            "nvme controller doorbells starting at phys {:p} mapped to {:p}",
            doorbell_frame.start_address(),
            doorbell_base
        );

        let (sub_tail, comp_head) = unsafe {
            // Safety: we just mapped the doorbell memory as part of properties.
            // We only create 1 queue with index 0 (admin) right here.
            // We check the stride later on, once we have proper access to the properties.
            CommandQueue::get_doorbells(doorbell_base, 4, 0)
        };

        // TODO figure out good admin queue size
        const ADMIN_QUEUE_SIZE: u16 = (Size4KiB::SIZE / SUBMISSION_COMMAND_ENTRY_SIZE) as u16;
        let admin_queue =
            CommandQueue::allocate(ADMIN_QUEUE_SIZE, ADMIN_QUEUE_SIZE, sub_tail, comp_head)?;

        let mut this = Self {
            pci_dev,
            controller_base_paddr: properties_base_paddr,
            controller_page: properties_page,
            controller_base_vaddr: properties_base_vaddr,
            doorbell_page,
            doorbell_base_vaddr: doorbell_base,
            admin_queue,
            controller_id: None,
            io_command_sets: IOCommandSetVector::zero(),
            max_number_completions_queues: 0,
            max_number_submission_queues: 0,
        };

        this.ensure_disabled();

        // 1. The host waits for the controller to indicate that any previous reset is complete by waiting for
        // CSTS.RDY to become ‘0’;
        while this.read_controller_status().ready != false {
            spin_loop();
        }
        let cap = this.read_capabilities();

        // 2. The host configures the Admin Queue by setting the Admin Queue Attributes (AQA), Admin
        // Submission Queue Base Address (ASQ), and Admin Completion Queue Base Address (ACQ) to
        // appropriate values;
        {
            trace!("writting admin queue properties: {:#?}", this.admin_queue);
            let mut aqa = this.read_aqa();
            aqa.submission_queue_size = this.admin_queue.submission_queue_size;
            aqa.completion_queue_size = this.admin_queue.completion_queue_size;
            this.write_aqa(aqa);

            let mut asq = this.read_asq();
            asq.paddr = this.admin_queue.submission_queue_paddr;
            this.write_asq(asq);

            let mut acq = this.read_acq();
            acq.paddr = this.admin_queue.completion_queue_paddr;
            this.write_acq(acq);
        }
        // ensure admin head and tail were set using the correct stride
        if cap.doorbell_stride != 4 {
            trace!("doorbell stride is not 4. This is fine, it just means we need to readjust the admin queue doorbells!");
            // We guessed wrong
            let (sub_tail, comp_head) = unsafe {
                // Safety: we just mapped the doorbell memory as part of properties.
                // we overwrite the old references, thereby ensuring no aliasing is done
                CommandQueue::get_doorbells(doorbell_base, cap.doorbell_stride, 0)
            };
            this.admin_queue.submission_queue_tail_doorbell = sub_tail;
            this.admin_queue.completion_queue_head_doorbell = comp_head;
        }

        // 3. The host determines the supported I/O Command Sets by checking the state of CAP.CSS and
        //  appropriately initializing CC.CSS as follows:
        //     a. If the CAP.CSS bit 7 is set to ‘1’, then the CC.CSS field should be set to 111b;
        //     b. If the CAP.CSS bit 6 is set to ‘1’, then the CC.CSS field should be set to 110b; and
        //     c. If the CAP.CSS bit 6 is cleared to ‘0’ and bit 0 is set to ‘1’, then the CC.CSS field should be set
        //          to 000b;
        let mut cc = this.read_configuration();
        if cap.command_sets_supported.get_bit(7) {
            cc.command_set_selected = 0b111;
        } else if cap.command_sets_supported.get_bit(6) {
            cc.command_set_selected = 0b110;
        } else if cap.command_sets_supported.get_bit(0) {
            cc.command_set_selected = 0b000;
        } else {
            return Err(NVMEControllerError::DeviceNotSupported(
                "No I/O command set found",
            ));
        }

        // 4. The controller settings should be configured. Specifically:
        //      a. The arbitration mechanism should be selected in CC.AMS; and
        //      b. The memory page size should be initialized in CC.MPS;
        cc.arbitration_mechanism = ArbitrationMechanism::RoundRobbin;
        cc.memory_page_size = Size4KiB::SIZE as u32;
        this.write_configuration(cc.clone());

        // 5. The host enables the controller by setting CC.EN to ‘1’;
        // NOTE: it is probably fine to set enabled with the same write we use to set the other
        // configs, but just to be save we use a second write
        cc.enable = true;
        this.write_configuration(cc);

        // 6. The host waits for the controller to indicate that the controller is ready to process commands. The
        // controller is ready to process commands when CSTS.RDY is set to ‘1’;
        while this.read_controller_status().ready != true {
            // TODO timeout: [Capabilities::timeout]
            spin_loop();
        }
        trace!("nvme device enabled!");

        // TODO I think I can remove this
        // ensure the head doorbell is 0
        debug!("enusre admin queue completion head is 0");
        this.admin_queue.completion_queue_head_doorbell.write(0);

        // 7. The host determines the configuration of the controller by issuing the Identify command specifying
        // the Identify Controller data structure (i.e., CNS 01h);
        let controller_id: ControllerId;
        {
            let identy_result_pt_flags =
                PageTableFlags::PRESENT | PageTableFlags::NO_CACHE | PageTableFlags::NO_EXECUTE;
            let (page, frame) = map_frame!(Size4KiB, identy_result_pt_flags)?;

            let command = admin_commands::create_identify_command(
                admin_commands::IdentifyNamespaceIdent::Controller,
                frame,
            );

            let ident = this.admin_queue.submit(command)?;
            this.admin_queue.flush();

            let completion = this.admin_queue.wait_for(ident).expect(
                "Wait for should always succeed because there is exactly 1 open submission",
            );
            if completion.status_and_phase.status().is_err() {
                error!("identify controller command failed!");
                return Err(completion.status_and_phase.status().into());
            }
            unsafe {
                log_hex_dump(
                    "Identify data:",
                    log::Level::Debug,
                    module_path!(),
                    page.start_address(),
                    2048,
                );
            }

            // Safety: we mapped this page earlier
            let identify_data: &IdentifyControllerData = unsafe { &*page.start_address().as_ptr() };
            controller_id = identify_data.controller_id;
            trace!("NVMe controller id: {:?}", controller_id);

            if cap.command_sets_supported.get_bit(0) {
                // TODO get info about NVM IO command set. Do I need to do something here?
            }

            let (frame, _pt_flags) = unmap_page!(page)?;
            free_page!(page);
            unsafe {
                // frame is unmapped and no longer used
                free_frame!(Size4KiB, frame);
            }
        }
        this.controller_id = Some(controller_id);

        // 8. The host determines any I/O Command Set specific configuration information as follows:
        //      a. If the CAP.CSS bit 6 is set to ‘1’, then the host does the following:
        if cap.command_sets_supported.get_bit(6) {
            let identy_result_pt_flags =
                PageTableFlags::PRESENT | PageTableFlags::NO_CACHE | PageTableFlags::NO_EXECUTE;
            let (page, frame) = map_frame!(Size4KiB, identy_result_pt_flags)?;

            //     i.  Issue the Identify command specifying the Identify I/O Command Set data structure (CNS
            //         1Ch); and
            let command = admin_commands::create_identify_command(
                admin_commands::IdentifyNamespaceIdent::IOCommandSet { controller_id },
                frame,
            );
            let ident = this.admin_queue.submit(command)?;
            this.admin_queue.flush();
            let completion = this.admin_queue.wait_for(ident).expect(
                "Wait for should always succeed because there is exactly 1 open submission",
            );
            if completion.status_and_phase.status().is_err() {
                error!("identify IO Command Set command failed!");
                return Err(completion.status_and_phase.status().into());
            }

            let command_sets = unsafe {
                // Safety: we just mapped the page
                IOCommandSetVectorIterator::from_vaddr(page.start_address())
            };

            let rate_set = |set: IOCommandSetVector| {
                // select the command set with the most options
                let mut rating: u64 = 0;
                if set.nvm() {
                    rating += 3;
                }
                if set.key_value() {
                    rating += 2;
                }
                if set.zoned_namespace() {
                    rating += 1;
                }

                rating
            };
            let Some((set_index, best_set, _)) = command_sets
                .map(|(idx, set)| {
                    trace!("IO Command Set {set:?} found at {idx}");
                    (idx, set)
                })
                .map(|(idx, set)| (idx, set, rate_set(set)))
                .max_by_key(|(_, _, rating)| *rating)
            else {
                return Err(NVMEControllerError::DeviceNotSupported(
                    "No supported IO Command Set found",
                ));
            };
            debug!("Active command set: {best_set:?}");
            this.io_command_sets = best_set;

            let (frame, _pt_flags) = unmap_page!(page)?;
            free_page!(page);
            unsafe {
                // frame is unmapped and no longer used
                free_frame!(Size4KiB, frame);
            }

            //     ii. Issue the Set Features command with the I/O Command Set Profile Feature Identifier (FID
            //         19h) specifying the index of the I/O Command Set Combination (refer to Figure 290) to be
            //         enabled; and
            let command = admin_commands::create_set_features_command(
                admin_commands::SetFeatureData::IOCommandSet { index: set_index },
                None,
            );
            let ident = this.admin_queue.submit(command)?;
            this.admin_queue.flush();

            this.admin_queue.wait_for(ident).expect(
                "Wait for should always succeed because there is exactly 1 open submission",
            );
            if completion.status_and_phase.status().is_err() {
                error!("set feature: IO Command Set command failed!");
                return Err(completion.status_and_phase.status().into());
            }
        } else if cap.command_sets_supported.get_bit(0) {
            this.io_command_sets.set_nvm(true);
        } else {
            return Err(NVMEControllerError::DeviceNotSupported(
                "only admin command set supported",
            ));
        }

        //      b. For each I/O Command Set that is enabled (Note: the NVM Command Set is enabled if the
        //         CC.CSS field is set to 000b):
        //          i.  Issue the Identify command specifying the I/O Command Set specific Active Namespace
        //              ID list (CNS 07h) with the appropriate Command Set Identifier (CSI) value of that I/O
        //              Command Set; and
        //          ii. For each NSID that is returned:
        //             1. If the enabled I/O Command Set is the NVM Command Set or an I/O Command Set
        //                based on the NVM Command Set (e.g., the Zoned Namespace Command Set) issue
        //                the Identify command specifying the Identify Namespace data structure (CNS 00h);
        //                and
        //             2. Issue the Identify command specifying each of the following data structures (refer to
        //                Figure 274): the I/O Command Set specific Identify Namespace data structure, the I/O
        //                Command Set specific Identify Controller data structure, and the I/O Command Set
        //                independent Identify Namespace data structure;
        todo_warn!("gather info about active command set");

        // 9. If the controller implements I/O queues, then the host should determine the number of I/O
        // Submission Queues and I/O Completion Queues supported using the Set Features command with
        // the Number of Queues feature identifier. After determining the number of I/O Queues, the NVMe
        // Transport specific interrupt registers (e.g. MSI and/or MSI-X registers) should be configured;
        {
            // convert into a 0 based value
            let request_count = io_queue_count_request.saturating_sub(1);

            let command = admin_commands::create_set_features_command(
                admin_commands::SetFeatureData::NumberOfQueues {
                    sub_count: request_count,
                    comp_count: request_count,
                },
                None,
            );

            let ident = this.admin_queue.submit(command)?;
            this.admin_queue.flush();

            let completion = this.admin_queue.wait_for(ident).expect(
                "Wait for should always succeed because there is exactly 1 open submission",
            );
            if completion.status_and_phase.status().is_err() {
                error!("set number of queue command failed!");
                return Err(completion.status_and_phase.status().into());
            }

            let completion_data = completion.dword0;
            this.max_number_submission_queues =
                (completion_data.get_bits(0..=15) + 1).try_into().unwrap();
            this.max_number_completions_queues =
                (completion_data.get_bits(16..=31) + 1).try_into().unwrap();

            if this.max_number_completions_queues < io_queue_count_request {
                warn!(
                    "Requested {} io completion queues, but device only supports {}",
                    io_queue_count_request, this.max_number_completions_queues
                );
            }
            if this.max_number_submission_queues < io_queue_count_request {
                warn!(
                    "Requested {} io submission queues, but device only supports {}",
                    io_queue_count_request, this.max_number_submission_queues
                );
            }
        }
        todo_error!("setup MSI and/or MSI-X");

        // 10. If the controller implements I/O queues, then the host should allocate the appropriate number of
        // I/O Completion Queues based on the number required for the system configuration and the number
        // supported by the controller. The I/O Completion Queues are allocated using the Create I/O
        // Completion Queue command;

        // 11. If the controller implements I/O queues, then the host should allocate the appropriate number of
        // I/O Submission Queues based on the number required for the system configuration and the number
        // supported by the controller. The I/O Submission Queues are allocated using the Create I/O
        // Submission Queue command; and

        // 12. To enable asynchronous notification of optional events, the host should issue a Set Features
        // command specifying the events to enable. To enable asynchronous notification of events, the host
        // should submit an appropriate number of Asynchronous Event Request commands. This step may
        // be done at any point after the controller signals that the controller is ready (i.e., CSTS.RDY is set
        // to ‘1’).
        todo_warn!("enable async notification events, eg for errors");

        Ok(this)
    }

    /// Ensures that CC.EN is set to 0
    ///
    /// if it is already disabled this does nothing,
    /// otherwise sets CC.EN to 0
    fn ensure_disabled(&mut self) {
        let mut config = self.read_configuration();
        if !config.enable {
            trace!("nvme device already disabled");
            return;
        }
        trace!("disable nvme device");
        config.enable = false;
        self.write_configuration(config);
    }

    /// Read a raw 32bit property
    fn read_property_32(&self, offset: u64) -> u32 {
        assert!(offset + 4 < Size4KiB::SIZE);
        let property = unsafe {
            // Safety: base_vaddr is mapped for 1 page and we have shared access to self
            (self.controller_base_vaddr + offset).as_volatile()
        };
        property.read()
    }

    fn write_property_32(&mut self, offset: u64, value: u32) {
        assert!(offset + 4 < Size4KiB::SIZE);
        let mut property = unsafe {
            // Safety: base_vaddr is mapped for 1 page and we have mutable access to self
            (self.controller_base_vaddr + offset).as_volatile_mut()
        };
        property.write(value)
    }

    /// Read a raw 64bit property
    fn read_property_64(&self, offset: u64) -> u64 {
        assert!(offset + 8 < Size4KiB::SIZE);
        let property = unsafe {
            // Safety: base_vaddr is mapped for 1 page and we have shared access to self
            (self.controller_base_vaddr + offset).as_volatile()
        };
        property.read()
    }

    fn write_property_64(&mut self, offset: u64, value: u64) {
        assert!(offset + 8 < Size4KiB::SIZE);
        let mut property = unsafe {
            // Safety: base_vaddr is mapped for 1 page and we have mutable access to self
            (self.controller_base_vaddr + offset).as_volatile_mut()
        };
        property.write(value)
    }

    pub fn read_capabilities(&self) -> Capabilites {
        self.read_property_64(0x0).into()
    }

    /// read the current controller status
    pub fn read_controller_status(&self) -> ControllerStatus {
        self.read_property_32(0x1c).into()
    }

    pub fn read_configuration(&self) -> ControllerConfiguration {
        self.read_property_32(0x14).into()
    }

    pub fn write_configuration(&mut self, configuration: ControllerConfiguration) {
        self.write_property_32(0x14, configuration.into())
    }

    /// read the current adming queue attributes
    fn read_aqa(&self) -> AdminQueueAttributes {
        self.read_property_32(0x24).into()
    }

    /// write the current adming queue attributes
    fn write_aqa(&mut self, aqa: AdminQueueAttributes) {
        self.write_property_32(0x24, aqa.into())
    }

    /// read the current admin submission queue address
    fn read_asq(&self) -> QueueBaseAddress {
        self.read_property_64(0x28).into()
    }

    /// write the current admin submission queue address
    fn write_asq(&mut self, asq: QueueBaseAddress) {
        self.write_property_64(0x28, asq.into())
    }

    /// read the current admin completion queue address
    fn read_acq(&self) -> QueueBaseAddress {
        self.read_property_64(0x30).into()
    }

    /// write the current admin completion queue address
    fn write_acq(&mut self, acq: QueueBaseAddress) {
        self.write_property_64(0x30, acq.into())
    }
}

#[derive_where(Debug)]
pub struct CommandQueue {
    submission_queue_size: u16,
    submission_queue_paddr: PhysAddr,
    submission_queue_vaddr: VirtAddr,

    /// dorbell that is written to to inform the controller that
    /// new command entries have been submited
    #[derive_where(skip)]
    submission_queue_tail_doorbell: Volatile<&'static mut u32, WriteOnly>,
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
    /// so we keep a copy of the value here.
    submission_queue_tail: u16,
    /// the last entry read by the controller.
    ///
    /// this value is set by the controller in each completion entry.
    submission_queue_head: u16,

    /// Indicates the index of the next "solt" to write a command entry
    /// in order to submit it to the controller. The slot is only free
    /// if the head is sufficiently ahead of the tail.
    submission_queue_tail_local: u16,

    completion_queue_size: u16,
    completion_queue_paddr: PhysAddr,
    completion_queue_vaddr: VirtAddr,

    /// dorbell that is written to to inform the controller that
    /// completion entries have been read, freeing the slots
    /// for the controller to fill with new completion entries
    #[derive_where(skip)]
    completion_queue_head_doorbell: Volatile<&'static mut u32, WriteOnly>,

    /// the next entry to read from this completion queue.
    ///
    /// [CommandQueue::completion_queue_head_doorbell] is writeonly
    /// so we keep a copy of the value here.
    completion_queue_head: u16,

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

    next_command_identifier: u16,
    // TODO implement drop and free memory
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
    unsafe fn get_doorbells(
        doorbell_base: VirtAddr,
        stride: u32,
        queue_index: u32,
    ) -> (
        Volatile<&'static mut u32, WriteOnly>,
        Volatile<&'static mut u32, WriteOnly>,
    ) {
        assert!(stride >= 4);

        let submission = doorbell_base + (queue_index as u64 * stride as u64);
        let completion = submission + stride as u64;

        trace!("doorbells: sub {:p}, comp {:p}", submission, completion);

        unsafe {
            // Safety: see outer function
            (
                submission.as_volatile_mut().write_only(),
                completion.as_volatile_mut().write_only(),
            )
        }
    }

    pub fn allocate(
        submission_queue_size: u16,
        completion_queue_size: u16,
        submission_tail_doorbell: Volatile<&'static mut u32, WriteOnly>,
        completion_head_doorbell: Volatile<&'static mut u32, WriteOnly>,
    ) -> Result<Self, NVMEControllerError> {
        if submission_queue_size < 2 {
            return Err(NVMEControllerError::InvalidQueueSize(submission_queue_size));
        }
        if completion_queue_size < 2 {
            return Err(NVMEControllerError::InvalidQueueSize(completion_queue_size));
        }

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

        let mut frame_allocator = WasabiFrameAllocator::<Size4KiB>::get_for_kernel().lock();
        let mut page_allocator = PageAllocator::get_kernel_allocator().lock();

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
            submission_queue_size,
            submission_queue_paddr,
            submission_queue_vaddr,
            completion_queue_size,
            completion_queue_paddr,
            completion_queue_vaddr,
            submission_queue_tail_doorbell: submission_tail_doorbell,
            submission_queue_tail: 0,
            submission_queue_tail_local: 0,
            submission_queue_head: 0,
            completion_queue_head_doorbell: completion_head_doorbell,
            completion_queue_head: 0,
            completion_expected_phase: true,
            completions: VecDeque::new(),
            next_command_identifier: 1,
        })
    }

    pub fn submit(
        &mut self,
        mut command: CommonCommand,
    ) -> Result<CommandIdentifier, NVMEControllerError> {
        if self.is_full_for_submission() {
            let new_completions = self
                .poll_completions()
                .unwrap_or_else(|(new_completions, _)| new_completions);
            if !new_completions || self.is_full_for_submission() {
                return Err(NVMEControllerError::QueueFull);
            }
        }

        trace!("submit command to queue");

        let identifier = CommandIdentifier(self.next_command_identifier);
        self.next_command_identifier = self.next_command_identifier.wrapping_add(1);

        command.dword0.set_command_identifier(identifier);

        let entry_slot_index = self.submission_queue_tail_local;
        // FIXME: wrapping behaviour is probably wrong when size = u16::max
        self.submission_queue_tail_local =
            self.submission_queue_tail_local.wrapping_add(1) % self.submission_queue_size;

        let slot_vaddr =
            self.submission_queue_vaddr + (SUBMISSION_COMMAND_ENTRY_SIZE * entry_slot_index as u64);
        unsafe {
            // Safety: submission queue is properly mapped to allow write access
            slot_vaddr
                .as_mut_ptr::<CommonCommand>()
                .write_volatile(command);
        }

        Ok(identifier)
    }

    /// Notify the controller about any pending submission command entries
    pub fn flush(&mut self) {
        if !self.has_submissions_pending_flush() {
            warn!("no submissions pending");
            return;
        }

        trace!(
            "flush command queue by writting {:#x} to doorbell",
            self.submission_queue_tail_local
        );

        self.submission_queue_tail_doorbell
            .write(self.submission_queue_tail_local as u32);
        self.submission_queue_tail = self.submission_queue_tail_local;
    }

    /// returns `true` if this queue is full for submissions
    ///
    /// Check completions to advance the last read submission entry
    /// of the controller
    ///
    /// See: NVMe Base Spec: 3.3.1.5: Full Queue
    pub fn is_full_for_submission(&self) -> bool {
        // FIXME: wrapping behaviour is probably wrong when size = u16::max
        // TODO unit test
        self.submission_queue_head
            == self.submission_queue_tail_local.wrapping_add(1) % self.submission_queue_size
    }

    /// returns `true` if there are no submission command entries pending
    ///
    /// See: NVMe Base Spec: 3.3.1.4: Empty Queue
    pub fn is_submissions_empty(&self) -> bool {
        self.submission_queue_tail_local == self.submission_queue_head
    }

    /// returns `true` as long as there are submission in the queue
    /// that the controller has not been notified about.
    ///
    /// Use [CommandQueue::flush] to notify the controller about new
    /// entries and clear this flag.
    pub fn has_submissions_pending_flush(&self) -> bool {
        self.submission_queue_tail != self.submission_queue_tail_local
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
    pub fn poll_completions(&mut self) -> Result<bool, (bool, CompletionPollError)> {
        trace!("poll for completions");
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
                .write(self.completion_queue_head as u32);
        }

        if let Some(err) = error {
            Err((any_found, err))
        } else {
            Ok(any_found)
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
    fn poll_single_completion(&mut self) -> Result<bool, CompletionPollError> {
        let slot_vaddr = self.completion_queue_vaddr
            + (COMPLETION_COMMAND_ENTRY_SIZE * self.completion_queue_head as u64);
        let possible_completion = unsafe {
            // Safety: completion queue is properly mapped for read access
            slot_vaddr.as_ptr::<CommonCompletionEntry>().read_volatile()
        };

        if possible_completion.status_and_phase.phase() != self.completion_expected_phase {
            // phase did not match, therefor this is the old completion entry
            return Ok(false);
        }

        // it is fine to update the submission head, even if we can not yet store this completion
        // because this only indicates that the controller has read the submission
        // command, not that it is fully handled
        //
        // the controller ensures this is wrapped around to 0 when neccessary.
        self.submission_queue_head = possible_completion.submission_queue_head;

        // binary search returns the index to insert at in the Err
        // if Ok the key is already in use, so we have to error
        //
        // we need to check this before incrementing the head index.
        // Otherwise this completion can never be read
        let Err(insert_at) = self
            .completions
            .binary_search_by_key(&possible_completion.command_ident, |c| c.command_ident)
        else {
            return Err(CompletionPollError::IdentifierStillInUse(
                possible_completion.command_ident,
            ));
        };

        // FIXME: wrapping behaviour on u16::max
        let mut next_head = self.completion_queue_head + 1;
        if next_head >= self.completion_queue_size {
            trace!("completios queue wrapping");
            // wrap head around to 0 and flip expected phase
            next_head = 0;
            self.completion_expected_phase = !self.completion_expected_phase;
        }
        self.completion_queue_head = next_head;

        self.completions.insert(insert_at, possible_completion);

        Ok(true)
    }

    pub fn iter_completions(&self) -> impl Iterator<Item = &CommonCompletionEntry> {
        self.completions.iter()
    }

    pub fn drain_completions(&mut self) -> impl Iterator<Item = CommonCompletionEntry> + '_ {
        self.completions.drain(0..)
    }

    pub fn wait_for(
        &mut self,
        ident: CommandIdentifier,
    ) -> Result<CommonCompletionEntry, CompletionPollError> {
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
            match self.poll_completions() {
                Ok(true) => {
                    if let Some(entry) = get_if_exists(&mut self.completions) {
                        return Ok(entry);
                    }
                    // entry not in the new completions, cointinue waiting
                    spin_loop()
                }
                Ok(false) => {
                    // no new entries, continue waiting
                    spin_loop();
                }
                Err((new_entries, err)) => {
                    if new_entries {
                        if let Some(entry) = get_if_exists(&mut self.completions) {
                            return Ok(entry);
                        }
                    }
                    return Err(err);
                }
            }
        }
    }
}

#[allow(missing_docs)]
#[derive(Error, Debug, PartialEq, Eq)]
pub enum CompletionPollError {
    #[error("Failed to poll command, because the identifier {0:#x} is still active!")]
    IdentifierStillInUse(CommandIdentifier),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, PartialOrd, Ord)]
pub struct CommandIdentifier(u16);

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

/// Command layout shared by all commands
//
/// See: NVM Express Base Spec: Figure 88: Common Command Format
#[repr(C)]
#[derive(Clone, PartialEq, Eq, Default)]
pub struct CommonCommand {
    dword0: CDW0,
    // NSID
    namespace_ident: u32,
    dword2: u32,
    dword3: u32,
    metadata_ptr: u64,
    data_ptr: DataPtr,
    dword10: u32,
    dword11: u32,
    dword12: u32,
    dword13: u32,
    dword14: u32,
    dword15: u32,
}
const_assert_eq!(
    size_of::<CommonCommand>() as u64,
    SUBMISSION_COMMAND_ENTRY_SIZE
);

/// The first command dword in a [CommonCommand]
///
/// this implementation is shared between all commands
///
/// See: NVM Express Base Spec: Figure 87: Command Dword 0
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct CDW0(u32);

#[allow(dead_code)]
impl CDW0 {
    fn zero() -> Self {
        Self(0)
    }

    fn opcode(&self) -> u8 {
        self.0.get_bits(0..=7) as u8
    }

    fn set_opcode(&mut self, value: u8) {
        self.0.set_bits(0..=7, value as u32);
    }

    fn fuse(&self) -> u8 {
        self.0.get_bits(8..=9) as u8
    }

    fn set_fuse(&mut self, value: u8) {
        self.0.set_bits(8..=9, value as u32);
    }

    /// PSDT
    fn prp_or_sgl(&self) -> PrpOrSgl {
        (self.0.get_bits(14..=15) as u8).try_into().unwrap()
    }

    fn set_prp_or_sgl(&mut self, value: PrpOrSgl) {
        self.0.set_bits(14..=15, value as u8 as u32);
    }

    fn command_identifier(&self) -> CommandIdentifier {
        CommandIdentifier(self.0.get_bits(16..=31) as u16)
    }

    fn set_command_identifier(&mut self, value: CommandIdentifier) {
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
    prp_entry_1: PhysAddr,
    prp_entry_2: PhysAddr,
}

impl Default for DataPtr {
    fn default() -> Self {
        DataPtr {
            prp_entry_1: PhysAddr::zero(),
            prp_entry_2: PhysAddr::zero(),
        }
    }
}

/// Common layout shared ba all completion entries
///
/// See: NVM Express Base Spec: Figure 90: Common Completion Queue Entry Layout
#[repr(C)]
#[derive(Clone, PartialEq, Eq, Default)]
pub struct CommonCompletionEntry {
    dword0: u32,
    dword1: u32,
    submission_queue_head: u16,
    submission_queue_ident: u16,
    command_ident: CommandIdentifier,
    status_and_phase: StatusAndPhase,
}
const_assert_eq!(
    size_of::<CommonCompletionEntry>(),
    COMPLETION_COMMAND_ENTRY_SIZE as usize
);

/// Status and Phase of a [CommonCompletionEntry]
///
/// See: NVM Express Base Spec: Figure 93: Completion Queue Entry: Status Field
// This struct uses a u16. The spec combines this field, with the command identifier
// into a u32. Therefor bit 16 in the spec is represented as bit 0 in this struct.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusAndPhase(u16);

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

    // TODO this function should be part of the Completion
    pub fn status(&self) -> CommandStatusCode {
        match self.status_code_type() {
            0 => {
                if let Ok(status) = GenericCommandStatus::try_from(self.status_code()) {
                    CommandStatusCode::GenericStatus(status)
                } else {
                    CommandStatusCode::UnknownGenericStatus(self.status_code())
                }
            }
            1 => CommandStatusCode::CommandSpecificStatus(self.status_code()),
            2 => CommandStatusCode::MediaAndDataIntegrityError(self.status_code()),
            3 => CommandStatusCode::PathRelatedStatus(self.status_code()),
            4..=6 => CommandStatusCode::Reserved {
                typ: self.status_code_type(),
                status: self.status_code(),
            },
            7 => CommandStatusCode::VendorSpecific(self.status_code()),
            _ => panic!("branch should be unreachable for 3bit value"),
        }
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

pub fn experiment_nvme_device() {
    // TODO temp
    let mut pci = PCI_ACCESS.lock();

    let nvme_device = pci
        .devices
        .iter()
        .find(|dev| {
            matches!(
                dev.class,
                Class::Storage(StorageSubclass::NonVolatileMemory)
            )
        })
        .unwrap()
        .clone();

    let _nvme_controller = unsafe {
        // TODO: Safety: we don't care during experiments
        NVMEController::initialize(&mut pci, nvme_device, 0xffff)
    }
    .unwrap();

    todo_warn!("Drop nvme_controller, which currently leaks data and makes it impossible to recover it properly.");
}

fn get_controller_properties_address(pci: &mut PCIAccess, nvme: Device, function: u8) -> PhysAddr {
    const ADDR_64_VALUE: u32 = 0b10;
    const ADDR_SIZE_RANGE: core::ops::RangeInclusive<usize> = 1..=2;
    const ADDR_MASK: u64 = !0xf;

    let mlbar = pci
        .read32(RegisterAddress::from_addr(
            nvme.address,
            function,
            CommonRegisterOffset::Bar0.into(),
        ))
        .unwrap();

    if !mlbar.get_bits(ADDR_SIZE_RANGE) == ADDR_64_VALUE {
        // 32 bit address
        return PhysAddr::new((mlbar as u64) & ADDR_MASK);
    }

    let mubar = pci
        .read32(RegisterAddress::from_addr(
            nvme.address,
            function,
            CommonRegisterOffset::Bar1.into(),
        ))
        .unwrap();

    PhysAddr::new((mubar as u64) << 32 | ((mlbar as u64) & ADDR_MASK))
}

/// A Queue base address(physical)
///
/// This must be page aligned based on TODO(CC.MPS)
///
/// In the spec this represents multiple queue base address types:
/// See: NVM Express base Spec:
///     Figure 50: Offset 28h: ASQ
///     Figure 51: Offset 30h: ACQ
/// TODOIhow is this mapped to the IO Queue, right now I only have the admin queue definitons
pub struct QueueBaseAddress {
    pub paddr: PhysAddr,
    reserved: u16,
}

/// the lower 12 bits are reserved
const QUEUE_BASE_ADDR_MAKS: u64 = !0xfff;

impl From<u64> for QueueBaseAddress {
    fn from(value: u64) -> Self {
        let paddr = PhysAddr::new(value & QUEUE_BASE_ADDR_MAKS);
        let reserved = value.get_bits(0..=11) as u16;
        Self { paddr, reserved }
    }
}

impl Into<u64> for QueueBaseAddress {
    fn into(self) -> u64 {
        assert_eq!(self.paddr.as_u64() & !QUEUE_BASE_ADDR_MAKS, 0);
        self.paddr.as_u64() & QUEUE_BASE_ADDR_MAKS | (self.reserved as u64)
    }
}
