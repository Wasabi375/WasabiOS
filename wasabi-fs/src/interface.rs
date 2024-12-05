use core::{
    error::Error,
    mem::{self, size_of, transmute},
    ptr::NonNull,
};

use alloc::boxed::Box;

use crate::{
    blocks_required_for, fs_structs::NodePointer, BlockGroup, BlockSlice, BLOCK_SIZE, LBA,
};

pub trait BlockDevice {
    type BlockDeviceError: Error + Send + Sync + 'static;

    /// Returns the number of blocks in the [BlockDevice]
    fn max_block_count(&self) -> Result<u64, Self::BlockDeviceError>;

    /// Read a block from the device
    fn read_block(&self, lba: LBA) -> Result<Box<BlockSlice>, Self::BlockDeviceError>;

    /// Read multiple blocks from the device
    fn read_blocks(
        &self,
        start: LBA,
        block_count: u64,
    ) -> Result<Box<[u8]>, Self::BlockDeviceError>;

    /// Write a block to the device
    fn write_block(
        &mut self,
        lba: LBA,
        data: NonNull<BlockSlice>,
    ) -> Result<(), Self::BlockDeviceError>;

    /// Write multiple blocks to the device
    fn write_blocks(
        &mut self,
        start: LBA,
        // TODO can I use NonNull<[BlockSlice]> instead?
        data: NonNull<[u8]>,
    ) -> Result<(), Self::BlockDeviceError>;

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
        self.read_blocks(group.start, group.count())
    }

    /// Write data to a [BlockGroup]
    fn write_block_group(
        &mut self,
        group: BlockGroup,
        data: NonNull<[u8]>,
    ) -> Result<(), Self::BlockDeviceError> {
        assert!(data.len() as u64 <= group.count() * BLOCK_SIZE as u64);
        self.write_blocks(group.start, data)
    }

    /// Read `T` from [BlockDevice]
    ///
    /// # Safety
    ///
    /// the caller must ensure that it is safe to construct a `T` via copy
    /// and that the pointer points to a `T` on the [BlockDevice]
    unsafe fn read_pointer<T>(&self, ptr: NodePointer<T>) -> Result<T, Self::BlockDeviceError> {
        if size_of::<T>() <= BLOCK_SIZE {
            let data = self.read_block(ptr.lba)?;

            unsafe { Ok((data.as_ptr() as *const T).read()) }
        } else {
            let count = blocks_required_for!(type: T);
            let data = self.read_blocks(ptr.lba, count)?;
            unsafe { Ok((data.as_ptr() as *const u8 as *const T).read()) }
        }
    }
}
