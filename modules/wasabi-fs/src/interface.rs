use core::{error::Error, mem::size_of, ptr::NonNull};

use alloc::boxed::Box;
use log::error;

use crate::{
    BLOCK_SIZE, BlockGroup, BlockSlice, LBA, blocks_required_for,
    fs_structs::{BlockConstructable, DevicePointer},
};

#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum BlockDeviceOrMemError<BDError: Error + Send + Sync + 'static> {
    #[error("Block device error: {0}")]
    BlockDevice(#[from] BDError),
    #[error("Failed to allocate memory(RAM)")]
    Allocation,
}

pub struct WriteData<'a> {
    pub data: &'a [u8],

    pub old_block_start: &'a [u8],
    pub old_block_end: &'a [u8],
}

impl WriteData<'_> {
    pub fn total_len(&self) -> usize {
        self.data.len() + self.old_block_start.len() + self.old_block_end.len()
    }

    // TODO is_valid check should really be done in a "constructor"
    pub fn is_valid(&self) -> bool {
        if self.old_block_start.len() >= BLOCK_SIZE {
            error!(
                "WriteData.old_block_start({}) must be less than BLOCK_SIZE({}). Otherwise WriteData should start at a later block",
                self.old_block_start.len(),
                BLOCK_SIZE
            );
            return false;
        }

        if self.old_block_end.len() >= BLOCK_SIZE {
            error!(
                "WriteData.old_block_end({}) must be less than BLOCK_SIZE({}). Otherwise WriteData should end at a earlier block",
                self.old_block_end.len(),
                BLOCK_SIZE
            );
            return false;
        }

        if self.total_len() % BLOCK_SIZE != 0 {
            error!(
                "WriteData.total_len() ({}) should be a mutliple of BLOCK_SIZE({})",
                self.total_len(),
                BLOCK_SIZE
            );

            return false;
        }

        true
    }
}

// TODO why is data a NonNull and not just a ref?
pub trait BlockDevice {
    type BlockDeviceError: Error + Send + Sync + 'static;

    /// Returns the number of blocks in the [BlockDevice]
    fn max_block_count(&self) -> Result<u64, Self::BlockDeviceError>;

    /// Read a block from the device
    ///
    /// The resulting slice will always be algined on block boundaries.
    fn read_block(&self, lba: LBA) -> Result<Box<BlockSlice>, Self::BlockDeviceError>;

    /// Read multiple contigious blocks from the device
    fn read_blocks_contig(
        &self,
        start: LBA,
        block_count: u64,
    ) -> Result<Box<[u8]>, Self::BlockDeviceError>;

    /// Read multiple blocks from the device
    fn read_blocks<I>(
        &self,
        blocks: I,
    ) -> Result<Box<[u8]>, BlockDeviceOrMemError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone;

    /// Write a block to the device
    fn write_block(
        &mut self,
        lba: LBA,
        data: NonNull<BlockSlice>,
    ) -> Result<(), Self::BlockDeviceError>;

    /// Write multiple blocks to the device
    fn write_blocks_contig(
        &mut self,
        start: LBA,
        // TODO can I use NonNull<[BlockSlice]> instead?
        data: NonNull<[u8]>,
    ) -> Result<(), Self::BlockDeviceError>;

    /// Write multiple blocks to the device
    fn write_blocks<I>(
        &mut self,
        blocks: I,
        data: WriteData,
    ) -> Result<(), BlockDeviceOrMemError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone;

    /// Atomically read a block from the device
    ///
    /// If the device does not support atomic access, this should be implemented using a lock
    fn read_block_atomic(&self, lba: LBA) -> Result<Box<BlockSlice>, Self::BlockDeviceError>;

    /// Atomically write a block to the device
    ///
    /// If the device does not support atomic access, this should be implemented using a lock
    fn write_block_atomic(
        &mut self,
        lba: LBA,
        data: NonNull<BlockSlice>,
    ) -> Result<(), Self::BlockDeviceError>;

    /// Atomically compare and exchange a block on the device
    ///
    /// If the device does not support atomic access, this should be implemented using a lock
    fn compare_exchange_block(
        &mut self,
        lba: LBA,
        current: NonNull<BlockSlice>,
        new: NonNull<BlockSlice>,
    ) -> Result<Result<(), Box<BlockSlice>>, Self::BlockDeviceError>;

    /// Read a [BlockGroup] from the device
    fn read_block_group(&self, group: BlockGroup) -> Result<Box<[u8]>, Self::BlockDeviceError> {
        self.read_blocks_contig(group.start, group.count())
    }

    /// Write data to a [BlockGroup]
    fn write_block_group(
        &mut self,
        group: BlockGroup,
        data: NonNull<[u8]>,
    ) -> Result<(), Self::BlockDeviceError> {
        assert!(data.len() as u64 <= group.count() * BLOCK_SIZE as u64);
        self.write_blocks_contig(group.start, data)
    }

    /// Read `T` from [BlockDevice]
    ///
    /// # Safety
    ///
    /// We assume the pointer points to a `T` on the [BlockDevice] and that the
    /// data on the block device is not corrupted.
    /// There is no real way to guarantee this and this operation is always unsafe.
    fn read_pointer<T: BlockConstructable>(
        &self,
        ptr: DevicePointer<T>,
    ) -> Result<T, Self::BlockDeviceError> {
        if size_of::<T>() <= BLOCK_SIZE {
            let data = self.read_block(ptr.lba)?;

            unsafe { Ok((data.as_ptr() as *const T).read()) }
        } else {
            let count = blocks_required_for!(type: T);
            let data = self.read_blocks_contig(ptr.lba, count)?;
            unsafe { Ok((data.as_ptr() as *const T).read()) }
        }
    }
}

#[cfg(any(feature = "test", test))]
pub mod test {
    use alloc::boxed::Box;

    use thiserror::Error;

    use super::{BlockDevice, BlockDeviceOrMemError};

    #[derive(Debug, Clone, Copy)]
    pub struct TestBlockDevice;

    #[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
    #[error("Test Block device should never be accessed")]
    pub struct TestBlockDeviceError;

    impl BlockDevice for TestBlockDevice {
        type BlockDeviceError = TestBlockDeviceError;

        fn max_block_count(&self) -> Result<u64, Self::BlockDeviceError> {
            Ok(0)
        }

        fn read_block(
            &self,
            _lba: crate::LBA,
        ) -> Result<Box<crate::BlockSlice>, Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn read_blocks_contig(
            &self,
            _start: crate::LBA,
            _block_count: u64,
        ) -> Result<Box<[u8]>, Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn write_block(
            &mut self,
            _lba: crate::LBA,
            _data: core::ptr::NonNull<crate::BlockSlice>,
        ) -> Result<(), Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn write_blocks_contig(
            &mut self,
            _start: crate::LBA,
            // TODO can I use NonNull<[BlockSlice]> instead?
            _data: core::ptr::NonNull<[u8]>,
        ) -> Result<(), Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn read_block_atomic(
            &self,
            _lba: crate::LBA,
        ) -> Result<Box<crate::BlockSlice>, Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn write_block_atomic(
            &mut self,
            _lba: crate::LBA,
            _data: core::ptr::NonNull<crate::BlockSlice>,
        ) -> Result<(), Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn compare_exchange_block(
            &mut self,
            _lba: crate::LBA,
            _current: core::ptr::NonNull<crate::BlockSlice>,
            _new: core::ptr::NonNull<crate::BlockSlice>,
        ) -> Result<Result<(), Box<crate::BlockSlice>>, Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn read_blocks<I>(
            &self,
            _blocks: I,
        ) -> Result<Box<[u8]>, BlockDeviceOrMemError<Self::BlockDeviceError>>
        where
            I: Iterator<Item = crate::BlockGroup> + Clone,
        {
            Err(TestBlockDeviceError.into())
        }

        fn write_blocks<I>(
            &mut self,
            _blocks: I,
            _data: super::WriteData,
        ) -> Result<(), BlockDeviceOrMemError<Self::BlockDeviceError>>
        where
            I: Iterator<Item = crate::BlockGroup> + Clone,
        {
            Err(TestBlockDeviceError.into())
        }
    }
}
