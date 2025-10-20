//! Library containing Structs and Traits used to Access a BlockDevice
//!
//! A block device is a virtual or hardware device that can be read and written
//! to in blocks of a fixed size.

#![no_std]
#![allow(incomplete_features)] // for generic_const_exprs
#![feature(arbitrary_self_types, box_as_ptr, generic_const_exprs)]
#![warn(missing_docs)]

extern crate alloc;

use core::{error::Error, mem::size_of};

use log::error;

mod structs;
use shared::alloc_ext::alloc_buffer_aligned;
pub use structs::*;

#[cfg(test)]
mod test_block_types {
    use super::block_size_types;

    use core::alloc::Layout;

    block_size_types!(4096: Block, BlockSlice);

    #[test]
    fn size_of() {
        assert_eq!(4096, core::mem::size_of::<Block<[u8; 16]>>());
    }

    #[test]
    fn layout() {
        let layout = Layout::new::<Block<[u8; 16]>>();
        assert_eq!(4096, layout.size(), "size");
        assert_eq!(4096, layout.align(), "align");
    }
}

/// Create the types to describe a block containg a generic type, and a u8 slice that is block aligned
/// and covers an entire block.
#[macro_export]
macro_rules! block_size_types {
    ($size:literal: $block:ident, $slice:ident) => {
        /// Type alias for `BlockAligned<[u8; $size]>`
        ///
        /// see [$aligned], [$block]
        #[allow(unused)]
        pub type $slice = $block<[u8; $size]>;

        /// Align `T` on block boundaries and ensure it is padded to fill the entire block
        ///
        /// See [BLOCK_SIZE], [BlockAligned], [BlockSlice], [blocks_required_for]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(C, align($size))]
        #[allow(unused)]
        pub struct $block<T>
        where
            [(); $size - core::mem::size_of::<T>()]:,
        {
            /// The data stored within the block
            pub data: T,
            _pad: [u8; $size - core::mem::size_of::<T>()],
        }
        static_assertions::const_assert_eq!(core::mem::size_of::<$block<[u8; $size]>>(), $size);

        impl<T> $block<T>
        where
            [(); $size - core::mem::size_of::<T>()]:,
        {
            /// Create a new block aligned `T`
            #[allow(unused)]
            pub const fn new(data: T) -> Self {
                Self {
                    data: data,
                    _pad: [0; { $size - core::mem::size_of::<T>() }],
                }
            }

            /// Extract the inner value
            #[allow(unused)]
            pub fn into_inner(self) -> T {
                self.data
            }

            /// Creates a ptr to a slice covering the entire block.
            ///
            /// The size of the slice is greater or equal than the size of `T`
            /// and will always cover exactly 1 or multiple blocks.
            #[allow(unused)]
            pub fn block_data(&self) -> core::ptr::NonNull<$slice> {
                assert!(core::mem::size_of::<Self>() == $size);

                // TODO using this pointer to read or write past $size is invalid
                core::ptr::NonNull::from(self).cast()
            }
        }

        impl $slice {
            /// A zero filled block
            #[allow(unused)]
            pub const ZERO: $slice = $block::new([0u8; $size]);
        }

        impl<T> core::ops::Deref for $block<T>
        where
            [(); $size - core::mem::size_of::<T>()]:,
        {
            type Target = T;

            fn deref(&self) -> &Self::Target {
                &self.data
            }
        }

        impl<T> core::ops::DerefMut for $block<T>
        where
            [(); $size - core::mem::size_of::<T>()]:,
        {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.data
            }
        }
    };
}

/// Helper struct containing data that should be written to a [BlockDevice]
///
/// Assuming [Self::data] is written to an existing range of blocks, then [Self::old_block_start]
/// contains the start of the first block that is partially overritten. [Self::old_block_end]
/// similarly contains the end of the last block that is partially overritten.
///
/// The combined size of all 3 fields should be a multiple of the block-size of the
/// [BlockDevice] the data is written to.
///
/// After the write the blocks overritten should contain [Self::old_block_start] followed
/// by [Self::data] and end with [Self::old_block_end].
pub struct WriteData<'a> {
    /// The data to write to the device
    pub data: &'a [u8],

    /// The old data in the first written block, until the start of [Self::data]
    pub old_block_start: &'a [u8],
    /// The old data in the last block, after [Self::data]
    pub old_block_end: &'a [u8],
}

impl WriteData<'_> {
    /// The total length of the data
    ///
    /// This should be a multiple of the block-size of the target [BlockDevice]
    pub fn total_len(&self) -> usize {
        self.data.len() + self.old_block_start.len() + self.old_block_end.len()
    }

    /// Check that `self` is valid and can be written to a [BlockDevice] with the given
    /// `block_size`.
    // TODO is_valid check should really be done in a "constructor"
    pub fn is_valid_for(&self, block_size: usize) -> bool {
        if self.old_block_start.len() >= block_size {
            error!(
                "WriteData.old_block_start({}) must be less than block_size({}). Otherwise WriteData should start at a later block",
                self.old_block_start.len(),
                block_size
            );
            return false;
        }

        if self.old_block_end.len() >= block_size {
            error!(
                "WriteData.old_block_end({}) must be less than block_size({}). Otherwise WriteData should end at a earlier block",
                self.old_block_end.len(),
                block_size
            );
            return false;
        }

        if self.total_len() % block_size != 0 {
            error!(
                "WriteData.total_len() ({}) should be a mutliple of block_size({})",
                self.total_len(),
                block_size
            );

            return false;
        }

        true
    }
}

#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum ReadBlockDeviceError<BDError: Error + Send + Sync + 'static> {
    #[error("Block device error: {0}")]
    BlockDevice(#[from] BDError),
    #[error("Failed to allocate memory(RAM)")]
    Allocation,
}

#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum WriteBlockDeviceError<BDError: Error + Send + Sync + 'static> {
    #[error("Block device error: {0}")]
    BlockDevice(#[from] BDError),
    #[error("expected to write {expected} bytes but only got {size}")]
    NotEnoughData { expected: usize, size: usize },
}

#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum ReadPointerError<BDError: Error + Send + Sync + 'static> {
    #[error("Block device error: {0}")]
    BlockDevice(#[from] BDError),
    #[error("Failed to allocate memory(RAM)")]
    Allocation,
    #[error(
        "Tried to read type with a size of {expected} bytes, but block size is only {block_size}"
    )]
    NotEnoughData { block_size: usize, expected: usize },
}

#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum CompareExchangeError<BDError: Error + Send + Sync + 'static> {
    #[error("Block device error: {0}")]
    BlockDevice(#[from] BDError),
    #[error("compare exchange failed, because the data is outdated")]
    OutdatedData,
    #[error("all argument buffers must be exactly of block size")]
    InvalidBufferSize,
}

impl<BDError: Error + Send + Sync + 'static> From<ReadBlockDeviceError<BDError>>
    for ReadPointerError<BDError>
{
    fn from(value: ReadBlockDeviceError<BDError>) -> Self {
        match value {
            ReadBlockDeviceError::BlockDevice(e) => ReadPointerError::BlockDevice(e),
            ReadBlockDeviceError::Allocation => ReadPointerError::Allocation,
        }
    }
}

/// A marker trait describing structs that can be constructed from a [super::Block].
///
/// This implies that it is safe to construct the struct from a byte slice read
/// from any block device.
///
/// # Safety
///
/// This type must be constructable from any bit pattern
pub unsafe trait BlockConstructable {}

/// A block device
///
/// this represents a virtual or hardware device that can be read and written to
/// in fixed size blocks.
// TODO remove result size for reads. Reads must always read all requested blocks or fail
pub trait BlockDevice {
    /// A gerneric error returned by the block device
    type BlockDeviceError: Error + Send + Sync + 'static;

    /// The block size of the device
    const BLOCK_SIZE: usize;

    /// Returns the number of blocks in the [BlockDevice]
    fn max_block_count(&self) -> Result<u64, Self::BlockDeviceError>;

    /// Read a block from the device into `buffer`
    ///
    /// `buffer` must have a length of exactly `Self::BLOCK_SIZE`
    fn read_block(
        &self,
        lba: LBA,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>>;

    /// Read multiple contigious blocks from the device
    ///
    /// `buffer` must have a length of at least `Self::BLOCK_SIZE * block_count`
    fn read_blocks_contig(
        &self,
        start: LBA,
        block_count: u64,
        buffer: &mut [u8],
    ) -> Result<usize, ReadBlockDeviceError<Self::BlockDeviceError>>;

    /// Read multiple blocks from the device
    ///
    /// `buffer` must have a length of at least `Self::BLOCK_SIZE * block_count`
    /// where `block_count` is the number of blocks in `blocks`
    fn read_blocks<I>(
        &self,
        blocks: I,
        buffer: &mut [u8],
    ) -> Result<usize, ReadBlockDeviceError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone;

    /// Write a block to the device
    ///
    /// `data` must have a length of exactly `Self::BLOCK_SIZE`
    fn write_block(
        &mut self,
        lba: LBA,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>>;

    /// Write multiple blocks to the device
    ///
    /// `data` must have a length that is a multiple `Self::BLOCK_SIZE`
    fn write_blocks_contig(
        &mut self,
        start: LBA,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>>;

    /// Write multiple blocks to the device
    fn write_blocks<I>(
        &mut self,
        blocks: I,
        data: WriteData,
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone;

    /// Atomically read a block from the device
    ///
    /// `buffer` must have a length of exactly `Self::BLOCK_SIZE`
    ///
    /// If the device does not support atomic access, this should be implemented using a lock
    fn read_block_atomic(
        &self,
        lba: LBA,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>>;

    /// Atomically write a block to the device
    ///
    /// `data` must have a length of exactly `Self::BLOCK_SIZE`
    ///
    /// If the device does not support atomic access, this should be implemented using a lock
    fn write_block_atomic(
        &mut self,
        lba: LBA,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>>;

    /// Atomically compare and exchange a block on the device
    ///
    /// This will compare `current` with what is stored on the device. If they
    /// are equal `new` will be written to the device. Otherwise `current` will be updated
    /// with the data read from the device.
    ///
    /// Both `current` and `new` must have a length of exactly `Self::BLOCK_SIZE`
    ///
    /// If the device does not support atomic access, this should be implemented using a lock
    ///
    /// # Return Value
    ///
    /// 1. `Ok(())` if the compare exchange succeeded.
    /// 2. `Err(CompareExchangeError::Outdated)` if the block did not match `current`
    /// 3. `Err(Self::BlockDeviceError)` if accessing the device failed
    /// 4. `Err(other)` if any other error occured
    ///  
    /// see [CompareExchangeError]
    fn compare_exchange_block(
        &mut self,
        lba: LBA,
        current: &mut [u8],
        new: &[u8],
    ) -> Result<(), CompareExchangeError<Self::BlockDeviceError>>;

    /// Read a [BlockGroup] from the device
    fn read_block_group(
        &self,
        group: BlockGroup,
        buffer: &mut [u8],
    ) -> Result<usize, ReadBlockDeviceError<Self::BlockDeviceError>> {
        self.read_blocks_contig(group.start, group.count(), buffer)
    }

    /// Write data to a [BlockGroup]
    fn write_block_group(
        &mut self,
        group: BlockGroup,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>> {
        assert!(data.len() as u64 <= group.count() * Self::BLOCK_SIZE as u64);
        self.write_blocks_contig(group.start, data)
    }

    /// Read `T` from [BlockDevice]
    ///
    /// TODO provide variant that takes in a block buffer
    /// TODO this sould be unsafe
    ///
    /// # Safety
    ///
    /// We assume the pointer points to a `T` on the [BlockDevice] and that the
    /// data on the block device is not corrupted.
    /// There is no real way to guarantee this and this operation is always unsafe.
    fn read_pointer<T: BlockConstructable>(
        &self,
        ptr: DevicePointer<T>,
    ) -> Result<T, ReadPointerError<Self::BlockDeviceError>> {
        if !(size_of::<T>() <= Self::BLOCK_SIZE) {
            return Err(ReadPointerError::NotEnoughData {
                expected: size_of::<T>(),
                block_size: Self::BLOCK_SIZE,
            });
        }

        let mut data = alloc_buffer_aligned(Self::BLOCK_SIZE, align_of::<T>())
            .map_err(|_| ReadPointerError::Allocation)?;

        self.read_block(ptr.lba, &mut data)?;

        unsafe {
            // Safety:
            //  data is aligned for T and it is save to construct T from device read
            Ok(data.as_ptr().cast::<T>().read())
        }
    }
}

/// A [BlockDevice] with a fixed size
pub trait SizedBlockDevice: BlockDevice {
    /// The size in blocks
    fn size(&self) -> u64;

    /// the address of the last valid block on the device
    fn last_lba(&self) -> LBA {
        LBA::new(self.size() - 1).expect("size - 1 can never be max")
    }
}

#[cfg(any(feature = "test", test))]
#[allow(missing_docs)]
pub mod test {

    use thiserror::Error;

    use crate::{
        BlockConstructable, BlockGroup, CompareExchangeError, DevicePointer, LBA, ReadPointerError,
        WriteBlockDeviceError,
    };

    use super::{BlockDevice, ReadBlockDeviceError, WriteData};

    /// A test block device that errors on use
    #[derive(Debug, Clone, Copy)]
    pub struct TestBlockDevice;

    #[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
    #[error("Test Block device should never be accessed")]
    #[allow(missing_docs)]
    pub struct TestBlockDeviceError;

    block_size_types!(4096: TBlock, TBlockSlice);

    impl BlockDevice for TestBlockDevice {
        type BlockDeviceError = TestBlockDeviceError;

        const BLOCK_SIZE: usize = 4096;

        fn max_block_count(&self) -> Result<u64, Self::BlockDeviceError> {
            Ok(0)
        }

        fn read_block(
            &self,
            _lba: crate::LBA,
            _buffer: &mut [u8],
        ) -> Result<(), ReadBlockDeviceError<TestBlockDeviceError>> {
            Err(TestBlockDeviceError.into())
        }

        fn read_blocks_contig(
            &self,
            _start: crate::LBA,
            _block_count: u64,
            _buffer: &mut [u8],
        ) -> Result<usize, ReadBlockDeviceError<Self::BlockDeviceError>> {
            Err(TestBlockDeviceError.into())
        }

        fn write_block(
            &mut self,
            _lba: crate::LBA,
            _data: &[u8],
        ) -> Result<(), WriteBlockDeviceError<TestBlockDeviceError>> {
            Err(TestBlockDeviceError.into())
        }

        fn write_blocks_contig(
            &mut self,
            _start: crate::LBA,
            _data: &[u8],
        ) -> Result<(), WriteBlockDeviceError<TestBlockDeviceError>> {
            Err(TestBlockDeviceError.into())
        }

        fn read_block_atomic(
            &self,
            _lba: crate::LBA,
            _buffer: &mut [u8],
        ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>> {
            Err(TestBlockDeviceError.into())
        }

        fn write_block_atomic(
            &mut self,
            _lba: crate::LBA,
            _data: &[u8],
        ) -> Result<(), WriteBlockDeviceError<TestBlockDeviceError>> {
            Err(TestBlockDeviceError.into())
        }

        fn compare_exchange_block(
            &mut self,
            _lba: crate::LBA,
            _current: &mut [u8],
            _new: &[u8],
        ) -> Result<(), CompareExchangeError<TestBlockDeviceError>> {
            Err(TestBlockDeviceError.into())
        }

        fn read_blocks<I>(
            &self,
            _blocks: I,
            _buffer: &mut [u8],
        ) -> Result<usize, ReadBlockDeviceError<Self::BlockDeviceError>>
        where
            I: Iterator<Item = BlockGroup> + Clone,
        {
            Err(TestBlockDeviceError.into())
        }

        fn write_blocks<I>(
            &mut self,
            _blocks: I,
            _data: WriteData,
        ) -> Result<(), WriteBlockDeviceError<TestBlockDeviceError>>
        where
            I: Iterator<Item = BlockGroup> + Clone,
        {
            Err(TestBlockDeviceError.into())
        }
    }

    struct TestField(#[allow(dead_code)] u8);
    unsafe impl BlockConstructable for TestField {}

    #[test]
    fn compile_test() {
        let device = TestBlockDevice;

        let _ = device.read_pointer(DevicePointer::<TestField>::new(LBA::ZERO));

        let _ = read_from_device(&device);
    }

    fn read_from_device<D: BlockDevice>(
        device: &D,
    ) -> Result<TestField, ReadPointerError<D::BlockDeviceError>>
    where
        [(); <D as BlockDevice>::BLOCK_SIZE]:,
    {
        device.read_pointer(DevicePointer::new(LBA::ZERO))
    }
}
