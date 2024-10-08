//! NVME pci device
//!
//! The specification documents can be found at https://nvmexpress.org/specifications/
//TODO move this module out of pci?

#![allow(missing_docs)] // TODO temp

pub mod admin_commands;
mod generic_command;
pub mod io_commands;
pub mod properties;
pub mod queue;

pub use generic_command::{
    CommandIdentifier, CommandStatusCode, CommonCommand, CommonCompletionEntry,
    GenericCommandStatus,
};
use queue::{CommandQueue, PollCompletionError, QueueIdentifier};

use self::properties::{
    AdminQueueAttributes, Capabilites, ControllerConfiguration, ControllerStatus,
};
use super::{Class, PCIAccess, StorageSubclass};
use crate::{
    frames_required_for, free_frame, free_page, map_frame,
    mem::{
        frame_allocator::WasabiFrameAllocator, page_table::PageTableMapError, MemError, VirtAddrExt,
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
};
use admin_commands::{CompletionQueueCreationStatus, SubmissionQueueCreationStatus};
use alloc::vec::Vec;
use bit_field::BitField;
use core::{cmp::min, hint::spin_loop, sync::atomic::Ordering};
use derive_where::derive_where;
use log::error;
use shared::{
    alloc_ext::{Strong, Weak},
    sync::lockcell::LockCell,
};
use thiserror::Error;
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
    controller_base_vaddr: VirtAddr,
    doorbell_page: Page<Size4KiB>,
    doorbell_base_vaddr: VirtAddr,
    doorbell_stride: u64,
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

        // TODO figure out good admin queue size
        const ADMIN_QUEUE_SIZE: u16 = (Size4KiB::SIZE / SUBMISSION_COMMAND_ENTRY_SIZE) as u16;
        let admin_queue = unsafe {
            // Safety: we just mapped the doorbell memory as part of properties.
            // We only create 1 queue with index 0 (admin) right here.
            // We check the stride later on, once we have proper access to the properties.
            CommandQueue::allocate(
                QueueIdentifier(0),
                ADMIN_QUEUE_SIZE,
                ADMIN_QUEUE_SIZE,
                doorbell_base,
                4,
            )?
        };

        let mut this = Self {
            pci_dev,
            controller_base_paddr: properties_base_paddr,
            controller_page: properties_page,
            controller_base_vaddr: properties_base_vaddr,
            doorbell_page,
            doorbell_base_vaddr: doorbell_base,
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
                    doorbell_base,
                    cap.doorbell_stride as u64,
                    QueueIdentifier(0),
                )
            };
            this.admin_queue.submission_queue_tail_doorbell = sub_tail;
            this.admin_queue.completion_queue_head_doorbell = comp_head;
            this.doorbell_stride = cap.doorbell_stride as u64;
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
        // 6. The host waits for the controller to indicate that the controller is ready to process commands. The
        // controller is ready to process commands when CSTS.RDY is set to ‘1’;
        this.ensure_enabled();

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
        let comp_size = Size4KiB::SIZE / COMPLETION_COMMAND_ENTRY_SIZE;
        let sub_size = Size4KiB::SIZE / SUBMISSION_COMMAND_ENTRY_SIZE;

        self.allocate_io_queues_with_sizes(
            count,
            min(comp_size.try_into().unwrap(), self.maximum_queue_entries),
            min(sub_size.try_into().unwrap(), self.maximum_queue_entries),
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

        // TODO: possible off by 1 error. Does this count include or exclude the admin queue
        if self
            .next_unused_io_queue_ident
            .checked_add(count)
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

        let mut frame_allocator = WasabiFrameAllocator::<Size4KiB>::get_for_kernel().lock();

        let comp_queue_requests = queue_idents
            .clone()
            .map(|ident| {
                let queue_size: u16 = completion_queue_size;

                let frame_count = frames_required_for!(
                    Size4KiB,
                    queue_size as u64 * COMPLETION_COMMAND_ENTRY_SIZE
                );
                assert!(frame_count >= 1);

                let Some(frames) = frame_allocator.alloc_range(frame_count) else {
                    return (ident, Err(MemError::OutOfMemory.into()));
                };
                let command = admin_commands::create_io_completion_queue(ident, queue_size, frames);

                let command_ident = self.admin_queue.submit(command);
                (ident, command_ident)
            })
            .collect::<Vec<_>>();
        drop(frame_allocator);

        if let Some(err) = comp_queue_requests
            .iter()
            .map(|(_, cmd)| cmd.clone())
            .filter_map(|cmd| cmd.err())
            .next()
        {
            error!("failed to submit create io completion queue commands");
            self.admin_queue.cancel_submissions();
            return Err(err);
        }

        self.admin_queue.flush();
        let mut comp_queue_requests_to_await: Vec<_> = comp_queue_requests
            .into_iter()
            .map(|(ident, request)| (ident, request.unwrap()))
            .collect();

        let mut comp_queue_creation_error = None;

        loop {
            let poll_result = self.admin_queue.poll_completions();
            if !poll_result.some_new_entries {
                if let Some(err) = poll_result.error_on_any_entry {
                    error!("failed to poll completions while creating io completion queues");
                    return Err(NVMEControllerError::AdminQueuePollCompletions(err));
                }
                spin_loop();
                continue;
            }

            for completion in self.admin_queue.iter_completions() {
                let Some(request_index) = comp_queue_requests_to_await
                    .iter()
                    .position(|(_, ci)| *ci == completion.command_ident)
                else {
                    continue;
                };

                let (queue_ident, _) = comp_queue_requests_to_await.swap_remove(request_index);

                if completion.status().is_err() {
                    let generic_status = completion.status();
                    if let CommandStatusCode::CommandSpecificStatus(status) = generic_status {
                        if let Some(comp_creation_error) =
                            CompletionQueueCreationStatus::from_bits(status)
                        {
                            error!("Failed to create completion queue {queue_ident:?}: {comp_creation_error:?}");
                        } else {
                            error!("Failed to create completion queue {queue_ident:?}: {generic_status:?}");
                        }
                    } else {
                        error!(
                            "Failed to create completion queue {queue_ident:?}: {generic_status}"
                        );
                    }
                    comp_queue_creation_error = Some(generic_status);
                }
            }

            // TODO timeout
            if comp_queue_requests_to_await.is_empty() {
                break;
            }
        }

        if let Some(err) = comp_queue_creation_error {
            todo_error!("delete all newly created io completion queues");
            return Err(NVMEControllerError::AdminCommandFailed(err));
        }

        let mut frame_allocator = WasabiFrameAllocator::<Size4KiB>::get_for_kernel().lock();

        let sub_queue_requests = queue_idents
            .clone()
            .map(|ident| {
                let queue_size: u16 = submission_queue_size;

                let frame_count = frames_required_for!(
                    Size4KiB,
                    queue_size as u64 * SUBMISSION_COMMAND_ENTRY_SIZE
                );
                assert!(frame_count >= 1);

                let Some(frames) = frame_allocator.alloc_range(frame_count) else {
                    return (ident, Err(MemError::OutOfMemory.into()));
                };
                let command = admin_commands::create_io_submission_queue(ident, queue_size, frames);

                let command_ident = self.admin_queue.submit(command);
                (ident, command_ident)
            })
            .collect::<Vec<_>>();
        drop(frame_allocator);

        if let Some(err) = sub_queue_requests
            .iter()
            .map(|(_, cmd)| cmd.clone())
            .filter_map(|cmd| cmd.err())
            .next()
        {
            error!("failed to submit create io submission queue commands");
            self.admin_queue.cancel_submissions();
            return Err(err);
        }

        self.admin_queue.flush();
        let mut sub_queue_requests_to_await: Vec<_> = sub_queue_requests
            .into_iter()
            .map(|(ident, request)| (ident, request.unwrap()))
            .collect();

        let mut sub_queue_creation_error = None;
        loop {
            let poll_result = self.admin_queue.poll_completions();
            if !poll_result.some_new_entries {
                if let Some(err) = poll_result.error_on_any_entry {
                    error!("failed to poll completions while creating io submission queues");
                    return Err(NVMEControllerError::AdminQueuePollCompletions(err));
                }
                spin_loop();
                continue;
            }

            for completion in self.admin_queue.iter_completions() {
                let Some(request_index) = sub_queue_requests_to_await
                    .iter()
                    .position(|(_, ci)| *ci == completion.command_ident)
                else {
                    continue;
                };

                let (queue_ident, _) = sub_queue_requests_to_await.swap_remove(request_index);

                if completion.status().is_err() {
                    let generic_status = completion.status();
                    if let CommandStatusCode::CommandSpecificStatus(status) = generic_status {
                        if let Ok(sub_creation_error) =
                            SubmissionQueueCreationStatus::try_from(status)
                        {
                            error!("Failed to create completion queue {queue_ident:?}: {sub_creation_error:?}");
                        } else {
                            error!("Failed to create completion queue {queue_ident:?}: {generic_status:?}");
                        }
                    } else {
                        error!(
                            "Failed to create completion queue {queue_ident:?}: {generic_status}"
                        );
                    }
                    sub_queue_creation_error = Some(generic_status);
                }
            }

            // TODO timeout
            if sub_queue_requests_to_await.is_empty() {
                break;
            }
        }

        if let Some(err) = sub_queue_creation_error {
            todo_error!("delete all newly created io submission and completion queues");
            return Err(NVMEControllerError::AdminCommandFailed(err));
        }

        let mut queues_to_add: Vec<CommandQueue> = Vec::with_capacity(count as usize);
        for ident in queue_idents {
            let queue = unsafe {
                // Safety:
                // doorbell_base_vaddr is properly mapped
                // we ensured that `ident` is only used for this queue
                CommandQueue::allocate(
                    ident,
                    submission_queue_size,
                    completion_queue_size,
                    self.doorbell_base_vaddr,
                    self.doorbell_stride,
                )
            };
            match queue {
                Ok(queue) => queues_to_add.push(queue),
                Err(err) => {
                    error!("failed to allocate command queue for {:?}", ident);
                    todo_error!("deallocate sub and comp queues");
                    return Err(err);
                }
            }
        }
        for queue in queues_to_add {
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
    }
}

pub fn experiment_nvme_device() {
    // TODO make into test
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
    {
        let mut nvme_controller = unsafe {
            // TODO: Safety: we don't care during experiments
            NVMEController::initialize(&mut pci, nvme_device, 0xffff)
        }
        .unwrap();

        nvme_controller
            .allocate_io_queues_with_sizes(4, 192, 16)
            .unwrap();

        assert_eq!(Ok(()), nvme_controller.ensure_available_io_queues(4));
        assert_eq!(Ok(()), nvme_controller.ensure_available_io_queues(8));

        debug!("should be save to drop");
        assert!(nvme_controller.is_safe_to_drop());
        core::sync::atomic::fence(Ordering::SeqCst);

        debug!("get queue");
        let queue = nvme_controller.get_io_queue().unwrap();

        core::sync::atomic::fence(Ordering::SeqCst);

        debug!("should not be save to drop now!");
        assert!(!nvme_controller.is_safe_to_drop());

        core::sync::atomic::fence(Ordering::SeqCst);
        drop(queue);

        debug!("should be save to drop again");
        core::sync::atomic::fence(Ordering::SeqCst);
        assert!(nvme_controller.is_safe_to_drop());

        core::sync::atomic::fence(Ordering::SeqCst);
        let _queue = nvme_controller.get_io_queue().unwrap();
        debug!("ensure drop order means that Strong<queue> is dropped before controller");
    }
    debug!("ensure controller can be recrated after drop");
    let mut nvme_controller = unsafe {
        // TODO: Safety: we don't care during experiments
        NVMEController::initialize(&mut pci, nvme_device, 0xffff)
    }
    .unwrap();
    nvme_controller.ensure_available_io_queues(4).unwrap();
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
// TODOIhow is this mapped to the IO Queue, right now I only have the admin queue definitons
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

    use crate::pci::{Class, StorageSubclass, PCI_ACCESS};

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
        for _ in 0..20 {
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
}
