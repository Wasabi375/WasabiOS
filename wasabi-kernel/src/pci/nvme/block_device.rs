//! NVME [BlockDevice]
//!

use core::iter::once;

use alloc::vec::Vec;
use block_device::{
    BlockDevice, BlockGroup, LBA, ReadBlockDeviceError, WriteBlockDeviceError, WriteData,
};
use log::{debug, error, trace};
use shared::{MiB, alloc_ext::Strong, sync::lockcell::LockCell};
use staticvec::StaticVec;
use thiserror::Error;
use x86_64::structures::paging::{Mapper, Page, PageSize, PageTableFlags, PhysFrame, Size4KiB};

use crate::{
    mem::{
        MemError, frame_allocator::FrameAllocator, page_allocator::PageAllocator,
        page_table::PageTable, structs::Pages,
    },
    pages_required_for,
    pci::nvme::{
        CommandIdentifier,
        io_commands::create_read_command,
        prp::{PrpEntry, PrpList},
    },
    prelude::TicketLock,
};

use super::{
    CommandStatusCode, NVMEControllerError,
    capabilities::ControllerCapabilities,
    prp::Prp,
    queue::{CommandQueue, PollCompletionError},
};

#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum NvmeBlockError {
    #[error("controller error: {0}")]
    Controller(#[from] NVMEControllerError),
    #[error("poll completion error: {0}")]
    Poll(#[from] PollCompletionError),
    #[error("Nvme command failed with status {0:?}")]
    CommandCompletionStatus(CommandStatusCode),
    #[error("failed to allocate memory: {0}")]
    Mem(#[from] MemError),
}

const MAX_REUSE_PRP_PAGE_COUNT: usize = pages_required_for!(Size4KiB, MiB!(1)) as usize;

/// NVME [BlockDevice]
///
/// ### LBA / block count sapce conversion
///
/// it is possible that the [BlockDevice::BLOCK_SIZE] does not match the
/// `lba_block_size` used by the nvme device. In that case it is necessary
/// to convert both LBAs and block-counts from "block-device" space into
/// "nvme-device" space.
/// This conversion also ensures that `LBA(0)` is mapped to [NvmeBlockDevice::start_lba]
///
/// All public functions should operate on block-device space values and convert into
/// nvme-device space when calling the internal implementations.
pub struct NvmeBlockDevice<const BLOCK_SIZE: usize = 4096> {
    /// the first block that can be written to
    ///
    /// this is used to subspace a device, e.g. full device vs partition 1
    ///
    /// this value is stored in block-device space
    start_lba: LBA,

    /// the number of blocks that can be accessed from this device
    ///
    /// this is used to subspace a device, e.g. full device vs partition 1
    ///
    /// this value is stored in block-device space
    block_count: u64,

    // TODO CommandQueue should not be stored in locks. Instead I should create a
    // new command queue for each core/process/something
    io_queue: TicketLock<Strong<CommandQueue>>,
    controller_cap: ControllerCapabilities,

    lba_count_conversion_shift: u32,

    prp_page_allocator: TicketLock<PrpPageAllocator>,
}

impl<const BLOCK_SIZE: usize> NvmeBlockDevice<BLOCK_SIZE> {
    pub fn new(
        block_count: u64,
        io_queue: Strong<CommandQueue>,
        controller_cap: ControllerCapabilities,
    ) -> Self {
        Self::new_with_offset(block_count, LBA::ZERO, io_queue, controller_cap)
    }

    pub fn new_with_offset(
        block_count: u64,
        first_lba: LBA,
        io_queue: Strong<CommandQueue>,
        controller_cap: ControllerCapabilities,
    ) -> Self {
        let block_size: u64 = Self::BLOCK_SIZE as u64;
        assert!(
            block_size.is_multiple_of(512),
            "Block size must be multiple of 512"
        );
        assert!(
            block_size.is_multiple_of(controller_cap.lba_block_size),
            "block-device size must be multiple of lba(nvme) block size"
        );
        assert!(controller_cap.lba_block_size <= block_size);

        assert_ne!(
            block_count
                .checked_add(first_lba.get())
                .map(|end| LBA::new(end))
                .flatten(),
            None,
            "Block count overflows LBA range from start"
        );

        assert_eq!(
            controller_cap.memory_page_size,
            Size4KiB::SIZE as u32,
            "only support 4k memory pages"
        );

        let lba_conversion_quotient = block_size / controller_cap.lba_block_size;
        let lba_conversion_shift = lba_conversion_quotient.ilog2();
        assert_eq!(
            controller_cap.lba_block_size << lba_conversion_shift,
            block_size
        );

        let this = Self {
            start_lba: first_lba,
            block_count,

            lba_count_conversion_shift: lba_conversion_shift,

            io_queue: TicketLock::new(io_queue),
            controller_cap,
            prp_page_allocator: TicketLock::new(PrpPageAllocator::empty()),
        };

        this.map_lba(this.start_lba)
            .get()
            .checked_add(this.map_block_count(this.block_count))
            .and_then(|end| LBA::new(end))
            .expect("Block count overflows LBA range from start(in nvme space)");

        this
    }

    fn map_lba(&self, input: LBA) -> LBA {
        input
            .get()
            // offset from start_lba
            .checked_add(self.start_lba.get())
            // convert into nvme space
            .and_then(|addr| addr.checked_shl(self.lba_count_conversion_shift))
            .and_then(|addr| LBA::new(addr))
            .expect("overflow lba during conversion from block-device to nvme space")
    }

    fn map_block_count(&self, count: u64) -> u64 {
        count
            .checked_shl(self.lba_count_conversion_shift)
            .expect("overflow block count during conversion from block-device to nvme space")
    }

    fn read_blocks_internal<I>(
        &self,
        blocks: I,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<NvmeBlockError>>
    where
        I: Iterator<Item = BlockGroup> + Clone,
    {
        fn free_prp<const BLOCK_SIZE: usize>(
            this: &NvmeBlockDevice<BLOCK_SIZE>,
            idents: &mut StaticVec<(CommandIdentifier, Prp), 64>,
        ) {
            let mut prp_allocator = this.prp_page_allocator.lock();
            for (_, prp) in idents.drain(..) {
                for (page, frame) in prp.mappings().iter().cloned() {
                    let free_result = unsafe {
                        // Safety: mapping is no longer used
                        prp_allocator.free(page, frame)
                    };
                    if let Err(free_err) = free_result {
                        error!(
                            "failed to free memory mapping for prp list, leaking memory: {free_err}"
                        );
                    }
                }
            }
        }

        fn flush_io_queue<const BLOCK_SIZE: usize>(
            this: &NvmeBlockDevice<BLOCK_SIZE>,
            io_queue: &mut CommandQueue,
            idents: &mut StaticVec<(CommandIdentifier, Prp), 64>,
        ) -> Result<(), ReadBlockDeviceError<NvmeBlockError>> {
            io_queue.flush();

            for (ident, _) in idents.iter() {
                let status = io_queue
                    .wait_for(*ident)
                    .map_err(NvmeBlockError::from)?
                    .status();

                if status.is_err() {
                    return Err(NvmeBlockError::CommandCompletionStatus(status).into());
                }
            }

            free_prp(this, idents);

            Ok(())
        }

        let mut remaining_buffer = buffer;
        let block_group_and_buf = blocks.map(|group| {
            let bytes = group.bytes(self.controller_cap.lba_block_size as usize) as usize;

            log::debug!(
                "splitting of {} bytes from buffer with {} bytes remaining",
                bytes,
                remaining_buffer.len()
            );

            let buf = remaining_buffer
                .split_off_mut(..bytes)
                .expect("buffer not large enough to store blocks");
            (group, buf)
        });

        let mut io_queue = self.io_queue.lock();

        let mut read_idents: StaticVec<(CommandIdentifier, Prp), 64> = StaticVec::new();

        let inner = || {
            for (block_group, buffer) in block_group_and_buf {
                let prp = self.create_prp_form_buffer(buffer)?;

                trace!(
                    "read {} blocks starting at {:?}",
                    block_group.count(),
                    block_group.start
                );

                let read_command = create_read_command(
                    &prp,
                    block_group.start,
                    block_group
                        .count()
                        .try_into()
                        .expect("block count should fit into u16"),
                );

                let read_ident = io_queue
                    .submit(read_command)
                    .map_err(NvmeBlockError::from)?;

                read_idents.push((read_ident, prp));

                if read_idents.is_full() || io_queue.is_full_for_submission() {
                    flush_io_queue(self, &mut io_queue, &mut read_idents)?;
                }
            }
            flush_io_queue(self, &mut io_queue, &mut read_idents)?;
            Ok::<(), ReadBlockDeviceError<NvmeBlockError>>(())
        };
        let inner_result = inner();

        free_prp(self, &mut read_idents);

        match inner_result {
            Ok(()) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn create_prp_form_buffer(
        &self,
        buffer: &mut [u8],
    ) -> Result<Prp, ReadBlockDeviceError<NvmeBlockError>> {
        let (pages, mut offset) = Pages::<Size4KiB>::from_ref(buffer);

        debug!("prep prp entries from buffer");
        let prp_entries: Vec<_> = {
            let page_table = PageTable::get_for_kernel().lock();
            pages
                .iter()
                .map(|page| {
                    page_table
                        .translate_page(page)
                        .expect("properly mapped buffer")
                })
                .map(|frame| {
                    PrpEntry::with_offset(frame, core::mem::take(&mut offset))
                        .expect("offset should be valid")
                })
                .inspect(|entry| trace!("prp entry: {entry:?}"))
                .collect() // TODO is there a collect that allows me to catch allocs errors?

            // TODO I don't like this alloc here but I also need to release the page-table lock
            // Not sure how I can solve this
        };

        match prp_entries.as_slice() {
            [] => todo!("this should be some kind of error"),
            [single_entry] => Ok(Prp::Entry(*single_entry)),
            [first, second] => Ok(Prp::DoubleEntry(*first, *second)),
            entries => {
                let prp_list = unsafe {
                    let mut prp_allocator = self.prp_page_allocator.lock();
                    // Safety: prp_page_allocator fullfills safety for prp list
                    PrpList::new(entries, || prp_allocator.allocate())
                        .map_err(NvmeBlockError::from)?
                };
                Ok(Prp::List(prp_list))
            }
        }
    }
}

#[expect(unused_variables)]
impl<const BLOCK_SIZE: usize> BlockDevice for NvmeBlockDevice<BLOCK_SIZE> {
    type BlockDeviceError = NvmeBlockError;

    const BLOCK_SIZE: usize = BLOCK_SIZE;

    fn size(&self) -> u64 {
        self.block_count
    }

    fn read_block(
        &self,
        lba: LBA,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>> {
        self.read_blocks_contig(lba, 1, buffer)
    }

    fn read_blocks_contig(
        &self,
        start: LBA,
        block_count: u64,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>> {
        let start = self.map_lba(start);
        let block_count = self.map_block_count(block_count);
        self.read_blocks_internal(
            once(BlockGroup::with_count(
                start,
                block_count.try_into().expect("Count must not be 0"),
            )),
            buffer,
        )
    }

    fn read_blocks<I>(
        &self,
        blocks: I,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone,
    {
        let blocks = blocks.map(|group| {
            BlockGroup::with_count(
                self.map_lba(group.start),
                self.map_block_count(group.count())
                    .try_into()
                    .expect("count was not 0 so mapping it should return non 0"),
            )
        });

        self.read_blocks_internal(blocks, buffer)
    }

    fn write_block(
        &mut self,
        lba: LBA,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>> {
        todo!()
    }

    fn write_blocks_contig(
        &mut self,
        start: LBA,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>> {
        todo!()
    }

    fn write_blocks<I>(
        &mut self,
        blocks: I,
        data: WriteData,
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone,
    {
        todo!()
    }

    fn read_block_atomic(
        &self,
        lba: LBA,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>> {
        todo!()
    }

    fn write_block_atomic(
        &mut self,
        lba: LBA,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>> {
        todo!()
    }

    fn compare_exchange_block(
        &mut self,
        lba: LBA,
        current: &mut [u8],
        new: &[u8],
    ) -> Result<(), block_device::CompareExchangeError<Self::BlockDeviceError>> {
        todo!()
    }
}

struct PrpPageAllocator {
    free_mappings: Vec<(Page<Size4KiB>, PhysFrame<Size4KiB>)>,
}

impl PrpPageAllocator {
    fn empty() -> Self {
        Self {
            free_mappings: Vec::new(),
        }
    }

    /// allocates a new prp page
    ///
    /// this means a [Page] that is mapped to a [PhysFrame] so that it can be used
    /// for a [PrpList](super::prp::PrpList)
    fn allocate(&mut self) -> Result<(Page<Size4KiB>, PhysFrame<Size4KiB>), MemError> {
        if let Some(mapping) = self.free_mappings.pop() {
            Ok(mapping)
        } else {
            let mut page_alloc = PageAllocator::get_for_kernel().lock();
            let mut frame_alloc = FrameAllocator::get_for_kernel().lock();

            let page = page_alloc.allocate_page_4k()?;
            let frame = frame_alloc.alloc_4k()?;

            // TODO do I need this to be no-chache for the prp list?
            let flags =
                PageTableFlags::NO_EXECUTE | PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

            unsafe {
                // Safety: this is the only mapping for this page/frame
                PageTable::get_for_kernel()
                    .lock()
                    .map_kernel(page, frame, flags, &mut frame_alloc)?
                    .flush();
            }

            Ok((page, frame))
        }
    }

    /// Free a mapping created using [Self::allocate]
    ///
    /// Safety:
    ///
    /// Follows [Self::free_mapping]
    unsafe fn free(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
    ) -> Result<(), MemError> {
        if self.free_mappings.len() < MAX_REUSE_PRP_PAGE_COUNT
            && self.free_mappings.try_reserve(1).is_ok()
        {
            self.free_mappings.push((page, frame));
            return Ok(());
        }

        unsafe {
            // Safety: we inherit the safety for free_mapping
            Self::free_mapping(page, frame)?;
        }

        Ok(())
    }

    /// unmap page and free both page and frame.
    ///
    /// panics if page is not mapped to frame.
    ///
    /// Safety:
    ///
    /// * `page` must be mapped to write to the memory backed by `frame`.
    /// * There must not be any pointers into page or any other mappaing to `frame`
    /// * Caller must not create any new mappings or pointers into `page` or `frame`
    unsafe fn free_mapping(
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
    ) -> Result<(), MemError> {
        let mut page_alloc = PageAllocator::get_for_kernel().lock();
        let mut frame_alloc = FrameAllocator::get_for_kernel().lock();

        match PageTable::get_for_kernel().lock().unmap(page) {
            Ok((free_frame, _flags, flush)) => {
                assert_eq!(frame, free_frame);
                flush.flush();
            }
            Err(err) => return Err(err.into()),
        }

        unsafe {
            // Safety: no pointers into page exist, because of function safety
            page_alloc.free_page(page);
            // Safety: only mapping to this page was just unmapped and the page freed
            frame_alloc.free(frame);
        }

        Ok(())
    }
}

impl Drop for PrpPageAllocator {
    fn drop(&mut self) {
        for (page, frame) in self.free_mappings.drain(..) {
            unsafe {
                // Safety:
                // based on the safety of `Self::free` there are no mappings
                // that would break the safety of `free_mapping` in the list
                if let Err(err) = Self::free_mapping(page, frame) {
                    error!("failed to free prp list mapping. Leaking memory: {err}");
                }
            }
        }
    }
}

#[cfg(feature = "test")]
pub mod test {
    use alloc::format;
    use block_device::{BlockDevice, BlockGroup, LBA};
    use shared::{alloc_ext::alloc_buffer_aligned, sync::lockcell::LockCell as _};
    use testing::{KernelTestError, TestUnwrapExt, kernel_test, t_assert_eq};

    use crate::pci::{PCI_ACCESS, nvme::NVMEController};

    use super::NvmeBlockDevice;

    #[repr(usize)]
    pub enum TestDrive {
        Boot = 0,
        Test = 1,
        NvmePattern = 2,
    }

    /// Creates a [NVMEController] and [NvmeBlockDevice] for testing
    ///
    /// # Safety
    ///
    /// must only called once while [NVMEController] is alive
    pub unsafe fn create_test_device(
        drive: TestDrive,
    ) -> Result<(NVMEController, NvmeBlockDevice<4096>), KernelTestError> {
        unsafe {
            // Safety: same as function
            create_test_device_with_size::<4096>(drive)
        }
    }

    /// Creates a [NVMEController] and [NvmeBlockDevice] for testing
    ///
    /// # Safety
    ///
    /// must only called once while [NVMEController] is alive
    pub unsafe fn create_test_device_with_size<const BLOCK_SIZE: usize>(
        drive: TestDrive,
    ) -> Result<(NVMEController, NvmeBlockDevice<BLOCK_SIZE>), KernelTestError> {
        let block_count = 10;

        let mut pci = PCI_ACCESS.lock();
        let mut devices = pci.devices.iter().filter(|dev| {
            matches!(
                dev.class,
                crate::pci::Class::Storage(crate::pci::StorageSubclass::NonVolatileMemory)
            )
        });
        let pci_dev = devices
            .nth(drive as usize)
            .cloned()
            .texpect("Failed to find pci_device")?;

        let mut controller =
            unsafe { NVMEController::initialize(pci.as_mut(), pci_dev, 0xffff) }.tunwrap()?;

        let io_queue = controller.get_or_alloc_io_queue().tunwrap()?;

        let device = NvmeBlockDevice::new(block_count, io_queue, controller.capabilities.clone());

        Ok((controller, device))
    }

    #[kernel_test]
    fn test_create_nvme_block_device() -> Result<(), KernelTestError> {
        let (_controller, _device) = unsafe {
            // Safety: only called once in this test
            create_test_device(TestDrive::Test)?
        };

        Ok(())
    }

    #[kernel_test]
    fn test_read_block() -> Result<(), KernelTestError> {
        let (_controller, device) = unsafe {
            // Safety: only called once in this test
            create_test_device(TestDrive::NvmePattern)?
        };

        let mut buffer = alloc_buffer_aligned(4096, 4096).tunwrap()?;

        device
            .read_block(LBA::ZERO, &mut buffer)
            .texpect("failed to read 0th block")?;

        let mut next_byte_expected: u8 = 0;
        const MAX_VALUE_EXPECTED: u8 = 251;

        for (index, byte) in buffer.iter().enumerate() {
            t_assert_eq!(*byte, next_byte_expected, "Byte at index {index} is wrong");
            next_byte_expected += 1;
            next_byte_expected %= MAX_VALUE_EXPECTED;
        }

        Ok(())
    }

    #[kernel_test]
    fn test_read_blocks_contig() -> Result<(), KernelTestError> {
        let (_controller, device) = unsafe {
            // Safety: only called once in this test
            create_test_device(TestDrive::NvmePattern)?
        };

        let mut buffer = alloc_buffer_aligned(4096 * 2, 4096).tunwrap()?;

        device
            .read_blocks_contig(LBA::ZERO, 2, &mut buffer)
            .texpect("failed to read 0th block")?;

        let mut next_byte_expected: u8 = 0;
        const MAX_VALUE_EXPECTED: u8 = 251;

        for (index, byte) in buffer.iter().enumerate() {
            t_assert_eq!(*byte, next_byte_expected, "Byte at index {index} is wrong");
            next_byte_expected += 1;
            next_byte_expected %= MAX_VALUE_EXPECTED;
        }

        Ok(())
    }

    #[kernel_test]
    fn test_read_block_offset_buffer() -> Result<(), KernelTestError> {
        let (_controller, device) = unsafe {
            // Safety: only called once in this test
            create_test_device(TestDrive::NvmePattern)?
        };

        for offset in [24, 1664] {
            let mut full_buffer = alloc_buffer_aligned(4096 + offset, 4096).tunwrap()?;
            let mut buffer = &mut full_buffer[offset..];

            device.read_block(LBA::ZERO, &mut buffer).texpect(&format!(
                "failed to read 0th block with buffer offset of {offset}"
            ))?;

            let mut next_byte_expected: u8 = 0;
            const MAX_VALUE_EXPECTED: u8 = 251;

            for (index, byte) in buffer.iter().enumerate() {
                t_assert_eq!(
                    *byte,
                    next_byte_expected,
                    "Byte at index {index} is wrong with buffer offset of {offset}"
                );
                next_byte_expected += 1;
                next_byte_expected %= MAX_VALUE_EXPECTED;
            }
        }

        Ok(())
    }

    #[kernel_test]
    fn test_read_blocks() -> Result<(), KernelTestError> {
        let (_controller, device) = unsafe {
            // Safety: only called once in this test
            create_test_device(TestDrive::NvmePattern)?
        };

        let mut buffer = alloc_buffer_aligned(4096 * 2, 4096).tunwrap()?;

        device
            .read_blocks(
                [
                    BlockGroup::single(LBA::ZERO),
                    BlockGroup::single(LBA::new(3).unwrap()),
                ]
                .iter()
                .cloned(),
                &mut buffer,
            )
            .tunwrap()?;

        let mut next_byte_expected: u8 = 0;
        const MAX_VALUE_EXPECTED: u8 = 251;

        for (index, byte) in buffer.iter().take(4096).enumerate() {
            t_assert_eq!(
                *byte,
                next_byte_expected,
                "Byte at index {index} in 0th block is wrong"
            );
            next_byte_expected += 1;
            next_byte_expected %= MAX_VALUE_EXPECTED;
        }

        next_byte_expected = ((4096u64 * 3) % MAX_VALUE_EXPECTED as u64)
            .try_into()
            .unwrap();

        for (index, byte) in buffer.iter().skip(4096).enumerate() {
            t_assert_eq!(
                *byte,
                next_byte_expected,
                "Byte at index {index} in 3th block is wrong"
            );
            next_byte_expected += 1;
            next_byte_expected %= MAX_VALUE_EXPECTED;
        }

        Ok(())
    }
}
