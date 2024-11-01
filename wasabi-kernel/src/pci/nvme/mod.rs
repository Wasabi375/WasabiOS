//! NVME pci device
//!
//! The specification documents can be found at https://nvmexpress.org/specifications/
//TODO move this module out of pci?

#![allow(missing_docs)] // TODO temp

pub mod admin_commands;
pub mod capabilities;
mod generic_command;
pub mod io_commands;
pub mod properties;
pub mod queue;

use super::{Class, PCIAccess, StorageSubclass};
use crate::{
    locals,
    mem::{
        frame_allocator::FrameAllocator,
        page_allocator::PageAllocator,
        page_table::{PageTable, PageTableKernelFlags, PageTableMapError},
        ptr::UntypedPtr,
        MemError,
    },
    pages_required_for,
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
    todo_error,
    utils::log_hex_dump,
};
use admin_commands::{CompletionQueueCreationStatus, IdentifyNamespaceData};
use alloc::vec::Vec;
use bit_field::BitField;
use capabilities::{ControllerCapabilities, Fuses, OptionalAdminCommands};
use core::{cmp::min, hint::spin_loop, sync::atomic::Ordering};
use derive_where::derive_where;
pub use generic_command::{
    CommandIdentifier, CommandStatusCode, CommonCommand, CommonCompletionEntry,
    GenericCommandStatus,
};
use generic_command::{COMPLETION_COMMAND_ENTRY_SIZE, SUBMISSION_COMMAND_ENTRY_SIZE};
use io_commands::LBA;
use properties::{AdminQueueAttributes, Capabilities, ControllerConfiguration, ControllerStatus};
use queue::{CommandQueue, PollCompletionError, QueueIdentifier};
use shared::{
    alloc_ext::{Strong, Weak},
    sync::lockcell::LockCell,
};
use thiserror::Error;
use x86_64::{
    structures::paging::{Mapper, Page, PageSize, PageTableFlags, PhysFrame, Size4KiB},
    PhysAddr,
};

#[allow(unused_imports)]
use crate::todo_warn;
#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

#[allow(missing_docs)]
#[derive(Debug, PartialEq, Error, Clone)]
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
    AdminCommandFailed(CommandStatusCode),
    #[error("Command queue is full")]
    QueueFull,
    #[error("Can't create {0} new io queues, because the maximum of {0} was reached")]
    IOQueueLimitReached(u16, u16),
    #[error("Failed to poll completions on the admin queue: {0}")]
    AdminQueuePollCompletions(PollCompletionError),
    #[error("No NVM Namespace found")]
    NoNamespace,
}

impl From<PageTableMapError> for NVMEControllerError {
    fn from(value: PageTableMapError) -> Self {
        let mem_err = MemError::from(value);
        mem_err.into()
    }
}

/// Provides communication with an NVME Storage Controller
#[derive_where(Debug)]
#[allow(dead_code)]
pub struct NVMEController {
    pci_dev: Device,
    controller_base_paddr: PhysAddr,
    controller_page: Page<Size4KiB>,
    controller_base_ptr: UntypedPtr,
    doorbell_page: Page<Size4KiB>,
    doorbell_base_ptr: UntypedPtr,
    doorbell_stride: isize,
    admin_queue: CommandQueue,

    controller_id: Option<ControllerId>,
    io_command_sets: IOCommandSetVector,

    max_number_completions_queues: u16,
    max_number_submission_queues: u16,

    #[derive_where(skip)]
    available_io_queues: Vec<Weak<CommandQueue>>,
    #[derive_where(skip)]
    used_io_queues: Vec<Weak<CommandQueue>>,

    // right now we assume that queues are never freed, and we can therefor just increment this.
    // If we ever allow for freeing queues we need to implement a better system to keep track of
    // the unused io queue identifiers, because this must never reach max_number_sub/comp_queues
    next_unused_io_queue_ident: QueueIdentifier,

    maximum_queue_entries: u16,

    capabilities: ControllerCapabilities,
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
    /// freed.
    /// Also this can not be freed untill all external access to any [CommandQueue] is dropped.
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

        info!("initializing NVMEController...");

        if pci_dev.functions > 1 {
            todo_warn!("support multi function nvme controllers not implemented!");
        }

        let mut page_allocator = PageAllocator::get_for_kernel().lock();
        let mut frame_allocator = FrameAllocator::get_for_kernel().lock();
        let mut page_table = PageTable::get_for_kernel().lock();

        let properties_base_paddr = get_controller_properties_address(pci, pci_dev, 0);

        let no_cache_page_flags: PageTableFlags = PageTableFlags::WRITABLE
            | PageTableFlags::PRESENT
            | PageTableFlags::NO_CACHE
            | PageTableFlags::NO_EXECUTE;

        let properties_frame = PhysFrame::containing_address(properties_base_paddr);
        let properties_frame_offset = properties_base_paddr - properties_frame.start_address();
        assert_eq!(
            properties_frame_offset, 0,
            "Properties that aren't page alligned are not supported"
        );
        let properties_page = page_allocator.allocate_page_4k()?;
        unsafe {
            // Safety: only called once for frame, assuming function safety
            page_table
                .map_to_with_table_flags(
                    properties_page,
                    properties_frame,
                    no_cache_page_flags,
                    PageTableFlags::KERNEL_TABLE_FLAGS,
                    frame_allocator.as_mut(),
                )
                .map_err(MemError::from)?
                .flush();
        };
        let properties_base_ptr = unsafe {
            // Safety: We just mapped the page
            UntypedPtr::new_from_page(properties_page)
                .expect("Allocated page should not be the 0 page")
                .add(properties_frame_offset as usize)
        };

        trace!(
            "nvme controller properties at phys {:p} mapped to {:p}",
            properties_base_paddr,
            properties_base_ptr
        );

        /// See NVME over PCIe Transport Spec: Figure 4: PCI Express Specific Property
        /// Definitions
        const DOORBELL_PHYS_OFFSET: u64 = 0x1000;
        let doorbell_frame =
            PhysFrame::from_start_address(properties_base_paddr + DOORBELL_PHYS_OFFSET).unwrap();
        let doorbell_page = page_allocator.allocate_page_4k()?;
        unsafe {
            // Safety: only called once for frame, assuming function safety
            page_table
                .map_to_with_table_flags(
                    doorbell_page,
                    doorbell_frame,
                    no_cache_page_flags,
                    PageTableFlags::KERNEL_TABLE_FLAGS,
                    frame_allocator.as_mut(),
                )
                .map_err(MemError::from)?
                .flush();
        };
        let doorbell_base_ptr: UntypedPtr = unsafe {
            UntypedPtr::new_from_page(doorbell_page)
                .expect("Allocated page should never be the 0 page")
        };
        trace!(
            "nvme controller doorbells starting at phys {:p} mapped to {:p}",
            doorbell_frame.start_address(),
            doorbell_base_ptr
        );

        // TODO figure out good admin queue size
        const ADMIN_QUEUE_SIZE: u16 =
            (Size4KiB::SIZE / SUBMISSION_COMMAND_ENTRY_SIZE as u64) as u16;
        let admin_queue = unsafe {
            // Safety: we just mapped the doorbell memory as part of properties.
            // We only create 1 queue with index 0 (admin) right here.
            // We check the stride later on, once we have proper access to the properties.
            CommandQueue::allocate(
                QueueIdentifier(0),
                ADMIN_QUEUE_SIZE,
                ADMIN_QUEUE_SIZE,
                doorbell_base_ptr,
                4,
                frame_allocator.as_mut(),
                page_allocator.as_mut(),
                page_table.as_mut(),
            )?
        };

        let mut this = Self {
            pci_dev,
            controller_base_paddr: properties_base_paddr,
            controller_page: properties_page,
            controller_base_ptr: properties_base_ptr,
            doorbell_page,
            doorbell_base_ptr,
            doorbell_stride: 4,
            admin_queue,
            controller_id: None,
            io_command_sets: IOCommandSetVector::empty(),
            max_number_completions_queues: 0,
            max_number_submission_queues: 0,
            available_io_queues: Vec::new(),
            used_io_queues: Vec::new(),
            next_unused_io_queue_ident: QueueIdentifier(1),
            maximum_queue_entries: 0, // this starts at 1, as the admin queue takes up slot 0
            capabilities: Default::default(),
        };

        // ensure admin head and tail were set using the correct stride
        let cap = this.read_capabilities();
        if cap.doorbell_stride != 4 {
            trace!("doorbell stride is not 4. This is fine, it just means we need to readjust the admin queue doorbells!");
            // We guessed wrong
            let (sub_tail, comp_head) = unsafe {
                // Safety: we just mapped the doorbell memory as part of properties.
                // we overwrite the old references, thereby ensuring no aliasing is done
                CommandQueue::get_doorbells(
                    doorbell_base_ptr,
                    cap.doorbell_stride as isize,
                    QueueIdentifier(0),
                )
            };
            this.admin_queue.submission_queue_tail_doorbell = sub_tail;
            this.admin_queue.completion_queue_head_doorbell = comp_head;
            this.doorbell_stride = cap.doorbell_stride as isize;
        }

        // 1. The host waits for the controller to indicate that any previous reset is complete by waiting for
        // CSTS.RDY to become ‘0’;
        this.ensure_disabled();

        let cap = this.read_capabilities();
        trace!("maximum queue entreies: {}", cap.maximum_queue_entries);
        this.maximum_queue_entries = cap.maximum_queue_entries;

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

        // 3. The host determines the supported I/O Command Sets by checking the state of CAP.CSS and
        //  appropriately initializing CC.CSS as follows:
        //     a. If the CAP.CSS bit 7 is set to ‘1’, then the CC.CSS field should be set to 111b;
        //     b. If the CAP.CSS bit 6 is set to ‘1’, then the CC.CSS field should be set to 110b; and
        //     c. If the CAP.CSS bit 6 is cleared to ‘0’ and bit 0 is set to ‘1’, then the CC.CSS field should be set
        //          to 000b;
        let mut cc = this.read_configuration();
        if cap.command_sets_supported.get_bit(7) {
            error!(
                "Only admin command set supported. We ignore this and try to use nvme IO anyways"
            );
            // cc.command_set_selected = 0b111;
            cc.command_set_selected = 0b000;
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
        this.capabilities.memory_page_size = cc.memory_page_size;
        this.write_configuration(cc.clone());

        // 5. The host enables the controller by setting CC.EN to ‘1’;
        // 6. The host waits for the controller to indicate that the controller is ready to process commands. The
        // controller is ready to process commands when CSTS.RDY is set to ‘1’;
        this.ensure_enabled();

        // 7. The host determines the configuration of the controller by issuing the Identify command specifying
        // the Identify Controller data structure (i.e., CNS 01h);
        let controller_id: ControllerId;
        {
            let identy_result_pt_flags =
                PageTableFlags::PRESENT | PageTableFlags::NO_CACHE | PageTableFlags::NO_EXECUTE;
            let page = page_allocator.allocate_page_4k()?;
            let frame = frame_allocator.alloc_4k()?;
            unsafe {
                // Safety: we just allocated frame and page
                page_table
                    .map_to_with_table_flags(
                        page,
                        frame,
                        identy_result_pt_flags,
                        PageTableFlags::KERNEL_TABLE_FLAGS,
                        frame_allocator.as_mut(),
                    )
                    .map_err(MemError::from)?
                    .flush();
            };

            let command = admin_commands::create_identify_command(
                admin_commands::ControllerOrNamespace::Controller,
                frame,
            );

            let ident = this.admin_queue.submit(command)?;
            this.admin_queue.flush();

            let completion = this.admin_queue.wait_for(ident).expect(
                "Wait for should always succeed because there is exactly 1 open submission",
            );
            if completion.status().is_err() {
                error!("identify controller command failed!");
                return Err(NVMEControllerError::AdminCommandFailed(completion.status()));
            }
            // Safety: we mapped this page earlier
            let identify_data: &IdentifyControllerData = unsafe { &*page.start_address().as_ptr() };
            controller_id = identify_data.controller_id;
            trace!("NVMe controller id: {:?}", controller_id);

            if cap.command_sets_supported.get_bit(0) {
                // TODO get info about NVM IO command set. Do I need to do something here?
            }

            this.capabilities.fuses = Fuses::from_bits_truncate(identify_data.fuses);
            this.capabilities.optional_admin_commands = OptionalAdminCommands::from_bits_truncate(
                identify_data.optional_admin_command_support,
            );

            let (frame, _pt_flags, flush) = page_table.unmap(page).map_err(MemError::from)?;
            flush.flush();
            unsafe {
                // frame and page is unmapped and no longer used
                page_allocator.free_page(page);
                frame_allocator.free(frame)
            }
        }
        this.controller_id = Some(controller_id);

        // 8. The host determines any I/O Command Set specific configuration information as follows:
        //      a. If the CAP.CSS bit 6 is set to ‘1’, then the host does the following:
        if cap.command_sets_supported.get_bit(6) {
            let identy_result_pt_flags =
                PageTableFlags::PRESENT | PageTableFlags::NO_CACHE | PageTableFlags::NO_EXECUTE;
            let page = page_allocator.allocate_page_4k()?;
            let frame = frame_allocator.alloc_4k()?;
            unsafe {
                // Safety: we just allocated frame and page
                page_table
                    .map_to_with_table_flags(
                        page,
                        frame,
                        identy_result_pt_flags,
                        PageTableFlags::KERNEL_TABLE_FLAGS,
                        frame_allocator.as_mut(),
                    )
                    .map_err(MemError::from)?
                    .flush();
            };

            //     i.  Issue the Identify command specifying the Identify I/O Command Set data structure (CNS
            //         1Ch); and
            let command = admin_commands::create_identify_command(
                admin_commands::ControllerOrNamespace::IOCommandSet { controller_id },
                frame,
            );
            let ident = this.admin_queue.submit(command)?;
            this.admin_queue.flush();
            let completion = this.admin_queue.wait_for(ident).expect(
                "Wait for should always succeed because there is exactly 1 open submission",
            );
            if completion.status().is_err() {
                error!("identify IO Command Set command failed!");
                return Err(NVMEControllerError::AdminCommandFailed(completion.status()));
            }

            let command_sets = unsafe {
                // Safety: we just mapped the page
                IOCommandSetVectorIterator::from_vaddr(page.start_address())
            };

            let rate_set = |set: IOCommandSetVector| {
                // select the command set with the most options
                let mut rating: u64 = 0;
                if set.contains(IOCommandSetVector::NVM) {
                    rating += 3;
                }
                if set.contains(IOCommandSetVector::KEY_VALUE) {
                    rating += 2;
                }
                if set.contains(IOCommandSetVector::ZONED_NAMESPACE) {
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

            let (frame, _pt_flags, flush) = page_table.unmap(page).map_err(MemError::from)?;
            flush.flush();
            unsafe {
                // page and frame is unmapped and no longer used
                page_allocator.free_page(page);
                frame_allocator.free(frame)
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
            if completion.status().is_err() {
                error!("set feature: IO Command Set command failed!");
                return Err(NVMEControllerError::AdminCommandFailed(completion.status()));
            }
        } else if cap.command_sets_supported.get_bit(0) {
            this.io_command_sets |= IOCommandSetVector::NVM;
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
        let mut cc = this.read_configuration();
        cc.io_submission_queue_entry_size = SUBMISSION_COMMAND_ENTRY_SIZE.try_into().unwrap();
        cc.io_completion_queue_entry_size = COMPLETION_COMMAND_ENTRY_SIZE.try_into().unwrap();
        todo_warn!("properly verify and set cc queue entry sizes");
        this.write_configuration(cc);

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
            if completion.status().is_err() {
                error!("set number of queue command failed!");
                return Err(NVMEControllerError::AdminCommandFailed(completion.status()));
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

        // NOTE: step 10 and 11 are done using the allocate_io_queues function

        // 12. To enable asynchronous notification of optional events, the host should issue a Set Features
        // command specifying the events to enable. To enable asynchronous notification of events, the host
        // should submit an appropriate number of Asynchronous Event Request commands. This step may
        // be done at any point after the controller signals that the controller is ready (i.e., CSTS.RDY is set
        // to ‘1’).
        todo_warn!("enable async notification events, eg for errors");

        // read namespace info to get block size
        // Also read list of active namespaces and assert that NSID is active.
        {
            debug!("Assume that NVM namespace 1 is valid and active");

            let identy_result_pt_flags =
                PageTableFlags::PRESENT | PageTableFlags::NO_CACHE | PageTableFlags::NO_EXECUTE;
            let page = page_allocator.allocate_page_4k()?;
            let frame = frame_allocator.alloc_4k()?;
            unsafe {
                // Safety: we just allocated frame and page
                page_table
                    .map_to_with_table_flags(
                        page,
                        frame,
                        identy_result_pt_flags,
                        PageTableFlags::KERNEL_TABLE_FLAGS,
                        frame_allocator.as_mut(),
                    )
                    .map_err(MemError::from)?
                    .flush();
            };

            let command = admin_commands::create_identify_command(
                admin_commands::ControllerOrNamespace::Namespace { nsid: 1 },
                frame,
            );

            let ident = this.admin_queue.submit(command)?;
            this.admin_queue.flush();
            let completion = this.admin_queue.wait_for(ident).expect(
                "Wait for should always succeed because there is exactly 1 open submission",
            );
            if completion.status().is_err() {
                error!("identify namespace 1 command failed!");
                return Err(NVMEControllerError::AdminCommandFailed(completion.status()));
            }

            // Safety: we mapped this page earlier
            let Some(identify_data) =
                (unsafe { IdentifyNamespaceData::from_vaddr(page.start_address()) })
            else {
                error!("Could not find NVM Namespace 1");
                return Err(NVMEControllerError::NoNamespace);
            };

            this.capabilities.lba_block_size = identify_data.active_lba_format().lba_data_size();
            this.capabilities.namespace_size_blocks = identify_data.namespace_size;
            this.capabilities.namespace_capacity_blocks = identify_data.namespace_capacity;
            this.capabilities.namespace_features = identify_data.features;

            if identify_data.active_lba_format().metadata_size != 0 {
                warn!(
                    "Metadata size is not 0: {}",
                    identify_data.active_lba_format().metadata_size
                );
            }

            // invalidate the reference into the page so we can reuse it
            let _ = identify_data;

            trace!("check that there is only 1 namespace");
            let command = admin_commands::create_identify_command(
                admin_commands::ControllerOrNamespace::Namespace { nsid: 2 },
                frame,
            );

            let ident = this.admin_queue.submit(command)?;
            this.admin_queue.flush();
            let completion = this.admin_queue.wait_for(ident).expect(
                "Wait for should always succeed because there is exactly 1 open submission",
            );
            if completion.status().is_err() {
                error!("identify namespace 2 command failed!");
                return Err(NVMEControllerError::AdminCommandFailed(completion.status()));
            }

            // Safety: we mapped this page earlier
            if unsafe { IdentifyNamespaceData::from_vaddr(page.start_address()) }.is_some() {
                warn!("There is a second NVM namespace on the device");
            }

            trace!("get active namesapce list");
            let command = admin_commands::create_identify_command(
                admin_commands::ControllerOrNamespace::ActiveNamespaces { starting_nsid: 0 },
                frame,
            );

            let ident = this.admin_queue.submit(command)?;
            this.admin_queue.flush();
            let completion = this.admin_queue.wait_for(ident).expect(
                "Wait for should always succeed because there is exactly 1 open submission",
            );
            if completion.status().is_err() {
                error!("identify active namespaces command failed!");
                return Err(NVMEControllerError::AdminCommandFailed(completion.status()));
            }

            // Safety: We mapped the page above and it is no longer used for the previous identify
            // command
            let active_namespaces: &[u32; 1024] = unsafe { &*page.start_address().as_ptr() };
            if active_namespaces[0] != 1 {
                error!("Namespace 1 is not active");
                return Err(NVMEControllerError::NoNamespace);
            }

            let (frame, _pt_flags, flush) = page_table.unmap(page).map_err(MemError::from)?;
            flush.flush();
            unsafe {
                // page and frame is unmapped and no longer used
                page_allocator.free_page(page);
                frame_allocator.free(frame)
            }
        }

        debug!("NVME Controller initizalized");
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

        core::sync::atomic::fence(Ordering::SeqCst);

        while self.read_controller_status().ready != false {
            spin_loop();
        }
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    /// Ensures that CC.EN is set to 1
    ///
    /// if it is already enable this does nothing,
    /// otherwise sets CC.EN to 1
    fn ensure_enabled(&mut self) {
        let mut config = self.read_configuration();
        if config.enable {
            trace!("nvme device already enabled");
            return;
        }
        trace!("enable nvme device");
        config.enable = true;
        self.write_configuration(config);

        core::sync::atomic::fence(Ordering::SeqCst);

        while self.read_controller_status().ready != true {
            spin_loop();
        }
        core::sync::atomic::fence(Ordering::SeqCst);
    }

    /// Read a raw 32bit property
    fn read_property_32(&self, offset: isize) -> u32 {
        assert!(offset + 4 < Size4KiB::SIZE as isize);
        let property = unsafe {
            // Safety: base_vaddr is mapped for 1 page and we have shared access to self
            self.controller_base_ptr.offset(offset).as_volatile()
        };
        property.read()
    }

    fn write_property_32(&mut self, offset: isize, value: u32) {
        assert!(offset + 4 < Size4KiB::SIZE as isize);
        let mut property = unsafe {
            // Safety: base_vaddr is mapped for 1 page and we have mutable access to self
            self.controller_base_ptr.offset(offset).as_volatile_mut()
        };
        property.write(value)
    }

    /// Read a raw 64bit property
    fn read_property_64(&self, offset: isize) -> u64 {
        assert!(offset + 8 < Size4KiB::SIZE as isize);
        let property = unsafe {
            // Safety: base_vaddr is mapped for 1 page and we have shared access to self
            self.controller_base_ptr.offset(offset).as_volatile()
        };
        property.read()
    }

    fn write_property_64(&mut self, offset: isize, value: u64) {
        assert!(offset + 8 < Size4KiB::SIZE as isize);
        let mut property = unsafe {
            // Safety: base_vaddr is mapped for 1 page and we have mutable access to self
            self.controller_base_ptr.offset(offset).as_volatile_mut()
        };
        property.write(value)
    }

    pub fn read_capabilities(&self) -> Capabilities {
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

    /// get an existing IO queue.
    ///
    /// This will return `None` if no queues exist or if all queues are already
    /// given.
    ///
    /// # Safety:
    ///
    /// the resulting [Strong] must be dropped before `self` can be dropped.
    pub fn get_io_queue(&mut self) -> Option<Strong<CommandQueue>> {
        self.available_io_queues
            .pop()
            .or_else(|| {
                self.reclaim_io_queues();
                self.available_io_queues.pop()
            })
            .map(|weak| {
                let strong = weak
                    .try_upgrade()
                    .expect("weak pointer in available_io_queues should always be upgradable");
                self.used_io_queues.push(weak);
                strong
            })
    }

    /// Tries to reclaim used io queues.
    ///
    /// A queue can be reclaimed if the [Strong] pointer to it is dropped
    pub fn reclaim_io_queues(&mut self) -> u16 {
        let mut index = 0;
        let mut reclamied = 0;
        while index < self.used_io_queues.len() {
            if let Some(_strong) = self.used_io_queues[index].try_upgrade() {
                let weak = self.used_io_queues.swap_remove(index);
                self.available_io_queues.push(weak);
                reclamied += 1;
            } else {
                index += 1;
            }
        }
        reclamied
    }

    /// Tries to get an existing io queue, but will allocate a new queue if none is available
    ///
    /// # Safety:
    ///
    /// the resulting [Strong] must be dropped before `self` can be dropped.
    pub fn get_or_alloc_io_queue(&mut self) -> Result<Strong<CommandQueue>, NVMEControllerError> {
        if let Some(queue) = self.get_io_queue() {
            return Ok(queue);
        }
        self.allocate_io_queues(1)?;
        Ok(self.get_io_queue().expect("Queue was just allocated"))
    }

    /// The maximum number of queues that can be created
    pub fn max_io_queue_count(&self) -> u16 {
        core::cmp::min(
            self.max_number_submission_queues,
            self.max_number_completions_queues,
        )
    }

    /// Ensures that `count` io queues are available.
    ///
    /// This will allocate up to `count` new queues if necessary.
    pub fn ensure_available_io_queues(&mut self, count: u16) -> Result<(), NVMEControllerError> {
        if self.available_io_queues.len() >= count.into() {
            return Ok(());
        }
        self.reclaim_io_queues();
        if self.available_io_queues.len() >= count.into() {
            return Ok(());
        }
        let to_alloc = count - (self.available_io_queues.len() as u16);
        assert!(to_alloc > 0);
        trace!("ensure available io queues needs to allocate");

        self.allocate_io_queues(to_alloc)
    }

    /// allocates `count` new IO queues.
    ///
    /// Failure means that the creation of 1 queue failed. The other queues might or
    /// might not have been created.
    ///
    /// The size of the queue is determined by the number of commands that fit into a single page
    pub fn allocate_io_queues(&mut self, count: u16) -> Result<(), NVMEControllerError> {
        const COMP_SIZE: u16 = (Size4KiB::SIZE / COMPLETION_COMMAND_ENTRY_SIZE as u64) as u16;
        const SUB_SIZE: u16 = (Size4KiB::SIZE / SUBMISSION_COMMAND_ENTRY_SIZE as u64) as u16;

        const QUEUE_SIZE: u16 = if COMP_SIZE < SUB_SIZE {
            COMP_SIZE
        } else {
            SUB_SIZE
        };

        self.allocate_io_queues_with_sizes(
            count,
            min(QUEUE_SIZE, self.maximum_queue_entries),
            min(QUEUE_SIZE, self.maximum_queue_entries),
        )
    }

    /// allocates `count` new IO queues.
    ///
    /// Failure means that the creation of 1 queue failed. The other queues might or
    /// might not have been created.
    pub fn allocate_io_queues_with_sizes(
        &mut self,
        count: u16,
        completion_queue_size: u16,
        submission_queue_size: u16,
    ) -> Result<(), NVMEControllerError> {
        trace!(
            "allocate_io_queues_with_sizes(count: {}, comp_size: {}, sub_size: {})",
            count,
            completion_queue_size,
            submission_queue_size
        );

        assert!(completion_queue_size <= self.maximum_queue_entries);
        assert!(submission_queue_size <= self.maximum_queue_entries);

        if self
            .next_unused_io_queue_ident
            .checked_add(count)
            // TODO: possible off by 1 error. Does this count include or exclude the admin queue
            .map(|requested_size| requested_size.as_u16() > self.max_io_queue_count())
            .unwrap_or(false)
        {
            return Err(NVMEControllerError::IOQueueLimitReached(
                count,
                self.max_io_queue_count(),
            ));
        }

        let queue_idents =
            (0..count).map(|ident_offset| self.next_unused_io_queue_ident + ident_offset);

        let mut page_alloc = PageAllocator::get_for_kernel().lock();
        let mut frame_alloc = FrameAllocator::get_for_kernel().lock();
        let mut page_table = PageTable::get_for_kernel().lock();

        let mut error = None;
        let queues: Vec<_> = queue_idents
            .map(|ident| unsafe {
                CommandQueue::allocate(
                    ident,
                    submission_queue_size,
                    completion_queue_size,
                    self.doorbell_base_ptr,
                    self.doorbell_stride,
                    &mut frame_alloc,
                    &mut page_alloc,
                    &mut page_table,
                )
            })
            .filter_map(|queue_or_err| match queue_or_err {
                Ok(queue) => Some(queue),
                Err(err) => {
                    error!("Failed to allocate IO-Command Queue: {err}");
                    if error.is_none() {
                        error = Some(err);
                    }
                    None
                }
            })
            .collect();
        if let Some(err) = error {
            return Err(err);
        }

        drop(page_alloc);
        drop(frame_alloc);
        drop(page_table);

        let create_comp_queue_commands = queues.iter().map(|queue| {
            admin_commands::create_io_completion_queue(
                queue.id(),
                queue.completion_queue_size,
                queue.completion_queue_paddr,
            )
        });
        let idents = match self.admin_queue.submit_all(create_comp_queue_commands) {
            Ok(idents) => idents,
            Err(err) => {
                error!("failed to submit command to create completion queue: {err}");
                self.admin_queue.cancel_submissions();
                return Err(err);
            }
        };
        self.admin_queue.flush();
        let create_comp_queue_results = match self.admin_queue.wait_for_all(idents) {
            Ok(ok) => ok,
            Err((_created, err)) => {
                todo_error!("delete all newly created io completion queues");
                return Err(NVMEControllerError::AdminQueuePollCompletions(err));
            }
        };

        if create_comp_queue_results
            .iter()
            .any(|res| res.status().is_err())
        {
            let mut error_status = None;
            for (failure, queue) in create_comp_queue_results
                .iter()
                .zip(queues.iter())
                .filter(|(res, _)| res.status().is_err())
            {
                let generic_status = failure.status();
                error_status = Some(generic_status);
                let queue_ident = queue.id();
                if let CommandStatusCode::CommandSpecificStatus(status) = generic_status {
                    if let Some(comp_creation_error) =
                        CompletionQueueCreationStatus::from_bits(status)
                    {
                        error!("Failed to create completion queue {queue_ident:?}: {comp_creation_error:?}");
                    } else {
                        error!(
                            "Failed to create completion queue {queue_ident:?}: {generic_status:?}"
                        );
                    }
                } else {
                    error!("Failed to create completion queue {queue_ident:?}: {generic_status}");
                }
            }

            todo_error!("delete all newly created io completion queues");

            let err = error_status.expect("We already check that there is an error");
            return Err(NVMEControllerError::AdminCommandFailed(err));
        }

        let create_sub_queue_commands = queues.iter().map(|queue| {
            admin_commands::create_io_submission_queue(
                queue.id(),
                queue.submission_queue_size,
                queue.submission_queue_paddr,
            )
        });
        let idents = match self.admin_queue.submit_all(create_sub_queue_commands) {
            Ok(idents) => idents,
            Err(err) => {
                error!("failed to submit command to create submission queue: {err}");
                self.admin_queue.cancel_submissions();
                return Err(err);
            }
        };
        self.admin_queue.flush();
        let create_sub_queue_results = match self.admin_queue.wait_for_all(idents) {
            Ok(ok) => ok,
            Err((_created, err)) => {
                todo_error!("delete all newly created io submission and completion queues");
                return Err(NVMEControllerError::AdminQueuePollCompletions(err));
            }
        };

        if create_sub_queue_results
            .iter()
            .any(|res| res.status().is_err())
        {
            let mut error_status = None;
            for (failure, queue) in create_sub_queue_results
                .iter()
                .zip(queues.iter())
                .filter(|(res, _)| res.status().is_err())
            {
                let generic_status = failure.status();
                error_status = Some(generic_status);
                let queue_ident = queue.id();
                if let CommandStatusCode::CommandSpecificStatus(status) = generic_status {
                    if let Some(sub_creation_error) =
                        CompletionQueueCreationStatus::from_bits(status)
                    {
                        error!("Failed to create submission queue {queue_ident:?}: {sub_creation_error:?}");
                    } else {
                        error!(
                            "Failed to create submission queue {queue_ident:?}: {generic_status:?}"
                        );
                    }
                } else {
                    error!("Failed to create submission queue {queue_ident:?}: {generic_status}");
                }
            }

            todo_error!("delete all newly created io submission and completion queues");

            let err = error_status.expect("We already check that there is an error");
            return Err(NVMEControllerError::AdminCommandFailed(err));
        }

        for queue in queues {
            self.available_io_queues.push(Weak::new(queue));
        }
        self.next_unused_io_queue_ident = self.next_unused_io_queue_ident + count;

        Ok(())
    }

    /// It is  only save to drop this, if no IO-queue is still in use.
    ///
    /// This will try to reclaim all remaining IO-queues. If this is successfull
    /// it is save to drop this, otherwise this must not be dropped.
    pub fn is_safe_to_drop(&mut self) -> bool {
        self.reclaim_io_queues();
        if self.used_io_queues.len() > 0 {
            false
        } else {
            true
        }
    }

    /// tries to drop this instance.
    ///
    /// This will return `Err(self)` if it is not save to drop `self`.
    ///
    /// See [Self::is_safe_to_drop].
    pub fn try_drop(mut self) -> Result<(), Self> {
        if !self.is_safe_to_drop() {
            return Err(self);
        }
        Ok(())
    }

    /// The [Capabilites] of the controller
    pub fn capabilities(&self) -> &ControllerCapabilities {
        &self.capabilities
    }
}

impl Drop for NVMEController {
    fn drop(&mut self) {
        if !self.is_safe_to_drop() {
            panic!("Tried to drop NVMEController while IO queues are still in use");
        }

        info!("Drop NVME Controller");

        // clear all old incomming completions
        self.admin_queue.clear_completions();

        let mut command_idents = Vec::with_capacity(2 * self.available_io_queues.len());

        // NOTE: we cant drain the queue, because that would drop the [CommandQueue] early and we
        // first need to delete the sub and comp queue on the nvme device. Therefor this cant be
        // done until after we polled all delete commands
        for weak_queue in self.available_io_queues.iter() {
            let queue = weak_queue
                .try_upgrade()
                .expect("We should have the only access to the queue at this point");

            let delete_sub = admin_commands::delete_io_submission_queue(queue.id());
            let delete_comp = admin_commands::delete_io_completion_queue(queue.id());

            command_idents.push(
                self.admin_queue
                    .submit(delete_sub)
                    .expect("Failed to submit delete submission queue command during drop"),
            );
            command_idents.push(
                self.admin_queue
                    .submit(delete_comp)
                    .expect("Failed to submit delete completion queue command during drop"),
            );
        }
        if !self.available_io_queues.is_empty() {
            self.admin_queue.flush();
        }

        loop {
            if let Some(err) = self.admin_queue.poll_completions().error_on_any_entry {
                panic!("failed to poll admin queue during drop: {err}");
            }
            for completion in self.admin_queue.drain_completions() {
                if let Some(pos) = command_idents
                    .iter()
                    .position(|ci| *ci == completion.command_ident)
                {
                    command_idents.swap_remove(pos);
                } else {
                    warn!(
                        "unexpected completion entry in admin queue during drop: Id: {:?}",
                        completion.command_ident
                    );
                }
            }
            if command_idents.is_empty() {
                break;
            }
        }
        trace!("all io queues deleted");

        let mut page_table = PageTable::get_for_kernel().lock();
        let mut page_allocator = PageAllocator::get_for_kernel().lock();

        match page_table.unmap(self.controller_page) {
            Ok((_, _, flush)) => {
                flush.flush();
                unsafe {
                    // Safety: pages are no longer used or accessible after drop
                    page_allocator.free_page(self.controller_page);
                }
            }
            Err(e) => {
                error!("Failed to unmap controller page on drop: {e:#?}");
            }
        }

        match page_table.unmap(self.doorbell_page) {
            Ok((_, _, flush)) => {
                flush.flush();
                unsafe {
                    // Safety: pages are no longer used or accessible after drop
                    page_allocator.free_page(self.doorbell_page);
                }
            }
            Err(e) => {
                error!("Failed to unmap doorbell page on drop: {e:#?}");
            }
        }
    }
}

pub fn experiment_nvme_device() {
    assert!(locals!().is_bsp());

    let mut pci = PCI_ACCESS.lock();

    let nvme_device = pci
        .devices
        .iter()
        .filter(|dev| {
            matches!(
                dev.class,
                Class::Storage(StorageSubclass::NonVolatileMemory)
            )
        })
        .nth(1)
        .cloned()
        .expect("Failed to find test nvme device");

    let mut nvme_controller = unsafe {
        // TODO: Safety: we don't care during experiments
        NVMEController::initialize(&mut pci, nvme_device, 0xffff)
    }
    .unwrap();

    info!("Controller cap: {:#?}", nvme_controller.capabilities());

    nvme_controller
        .allocate_io_queues_with_sizes(1, 64, 64)
        .expect("Failed to allocate nvme command queue");

    let mut io_queue = nvme_controller
        .get_io_queue()
        .expect("We juste allocated a queue so where is it?");

    let capabilities = nvme_controller.capabilities();

    let blocks_to_read = min(4, capabilities.namespace_size_blocks);

    let bytes = blocks_to_read * capabilities.lba_block_size;

    let page_count = pages_required_for!(Size4KiB, bytes);
    assert_eq!(page_count, 1, "TODO multipage not implemented");

    let pages;
    let frames;
    {
        let mut page_alloc = PageAllocator::get_for_kernel().lock();
        let mut frame_alloc = FrameAllocator::get_for_kernel().lock();

        pages = page_alloc.allocate_pages(page_count).unwrap();
        // TODO I don't think I need consecutive frames. PRP lists should support separate frames
        frames = frame_alloc.alloc_range(page_count).unwrap();

        let mut page_table = PageTable::get_for_kernel().lock();
        for (page, frame) in pages.iter().zip(frames) {
            unsafe {
                let flags = PageTableFlags::NO_EXECUTE | PageTableFlags::PRESENT;
                page_table
                    .map_to_with_table_flags(
                        page,
                        frame,
                        flags,
                        PageTableFlags::KERNEL_TABLE_FLAGS,
                        frame_alloc.as_mut(),
                    )
                    .unwrap()
                    .flush();
            }
        }
    }

    todo_warn!("Memory leak {page_count} pages for nvme read experiment");

    let command = io_commands::create_read_command(
        frames.start,
        LBA::new(0),
        blocks_to_read.try_into().unwrap(),
    );
    let ident = io_queue.submit(command).unwrap();

    io_queue.flush();

    let io_result = io_queue.wait_for(ident).unwrap();
    io_result.status().assert_success();

    unsafe {
        log_hex_dump(
            "NVME read first few blocks",
            log::Level::Info,
            module_path!(),
            UntypedPtr::new(pages.start_addr()).unwrap(),
            bytes as usize,
        );
    }
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
// TODO how is this mapped to the IO Queue, right now I only have the admin queue definitons
// TODO move this to properties module?
struct QueueBaseAddress {
    paddr: PhysAddr,
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

#[cfg(feature = "test")]
mod test {
    use shared::sync::lockcell::LockCell;
    use testing::{
        kernel_test, t_assert, t_assert_eq, t_assert_matches, KernelTestError, TestUnwrapExt,
    };
    use x86_64::structures::paging::{Mapper, PageTableFlags, Size4KiB};

    use crate::{
        mem::{
            frame_allocator::FrameAllocator,
            page_allocator::PageAllocator,
            page_table::{PageTable, PageTableKernelFlags},
            MemError,
        },
        pages_required_for,
        pci::{
            nvme::io_commands::{self, LBA},
            Class, StorageSubclass, PCI_ACCESS,
        },
        todo_warn,
    };

    use super::NVMEController;

    /// NOTE: device 0 is the boot device and should not be used
    /// for tests if possible.
    ///
    /// # Safety:
    ///
    /// must only be called once until result is dropped
    unsafe fn create_test_controller() -> Result<NVMEController, KernelTestError> {
        let mut pci = PCI_ACCESS.lock();
        let mut devices = pci.devices.iter().filter(|dev| {
            matches!(
                dev.class,
                Class::Storage(StorageSubclass::NonVolatileMemory)
            )
        });
        let pci_dev = devices
            .nth(1)
            .cloned()
            .texpect("Failed to find pci_device")?;

        unsafe { NVMEController::initialize(pci.as_mut(), pci_dev, 0xffff).tunwrap() }
    }

    #[kernel_test]
    fn test_find_all_nvme_devices() -> Result<(), KernelTestError> {
        let pci = PCI_ACCESS.lock();
        let mut devices = pci.devices.iter().filter(|dev| {
            matches!(
                dev.class,
                Class::Storage(StorageSubclass::NonVolatileMemory)
            )
        });
        t_assert_matches!(devices.next(), Some(_),);
        t_assert_matches!(devices.next(), Some(_));
        t_assert_matches!(devices.next(), None);
        Ok(())
    }

    #[kernel_test]
    fn test_create_and_drop_controller() -> Result<(), KernelTestError> {
        for _ in 0..3 {
            unsafe { create_test_controller() }.texpect("failed to create controller")?;
        }
        Ok(())
    }

    #[kernel_test]
    fn test_nvme_controller_save_to_drop() -> Result<(), KernelTestError> {
        let mut nvme_controller =
            unsafe { create_test_controller() }.texpect("failed to create controller")?;

        nvme_controller
            .allocate_io_queues_with_sizes(4, 192, 16)
            .unwrap();

        t_assert_eq!(Ok(()), nvme_controller.ensure_available_io_queues(4));
        t_assert_eq!(Ok(()), nvme_controller.ensure_available_io_queues(8));

        t_assert!(nvme_controller.is_safe_to_drop());
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        let queue = nvme_controller.get_io_queue().unwrap();

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        t_assert!(!nvme_controller.is_safe_to_drop());

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        drop(queue);

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        t_assert!(nvme_controller.is_safe_to_drop());

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        let _queue = nvme_controller.get_io_queue().unwrap();

        Ok(())
    }

    #[kernel_test(allow_page_leak, allow_frame_leak)]
    pub fn test_nvme_read_data() -> Result<(), KernelTestError> {
        let mut nvme_controller =
            unsafe { create_test_controller() }.texpect("failed to create controller")?;

        nvme_controller
            .allocate_io_queues_with_sizes(1, 64, 64)
            .texpect("Failed to allocate nvme command queue")?;

        let mut io_queue = nvme_controller
            .get_io_queue()
            .texpect("We juste allocated a queue so where is it?")?;

        let capabilities = nvme_controller.capabilities();

        let blocks_to_read = 1;

        let bytes = blocks_to_read * capabilities.lba_block_size;

        let page_count = pages_required_for!(Size4KiB, bytes);
        assert_eq!(page_count, 1, "TODO multipage not implemented");

        let mut page_alloc = PageAllocator::get_for_kernel().lock();
        let mut frame_alloc = FrameAllocator::get_for_kernel().lock();

        let pages = page_alloc
            .allocate_pages::<Size4KiB>(page_count)
            .tunwrap()?;
        // TODO I don't think I need consecutive frames. PRP lists should support separate frames
        let frames = frame_alloc.alloc_range(page_count).tunwrap()?;

        drop(page_alloc);

        for (page, frame) in pages.iter().zip(frames) {
            unsafe {
                let flags = PageTableFlags::NO_EXECUTE | PageTableFlags::PRESENT;
                let kernel_page_table = &mut PageTable::get_for_kernel().lock();
                let table_flags = PageTableFlags::KERNEL_TABLE_FLAGS;

                kernel_page_table
                    .map_to_with_table_flags(page, frame, flags, table_flags, frame_alloc.as_mut())
                    .map_err(MemError::from)
                    .tunwrap()?
                    .flush();
            }
        }

        drop(frame_alloc);

        todo_warn!("Memory leak {page_count} pages and frames for nvme read test");

        let command = io_commands::create_read_command(
            frames.start,
            LBA::new(0),
            blocks_to_read.try_into().unwrap(),
        );
        let ident = io_queue.submit(command).unwrap();

        io_queue.flush();

        let io_result = io_queue.wait_for(ident).unwrap();
        io_result.status().assert_success();

        // Safety: we just mapped this
        let read_data: &[u8; 4] = unsafe { &*pages.start_addr().as_ptr() };

        t_assert_eq!(read_data, b"test");

        Ok(())
    }
}
