//! Library containing Structs and Traits used to Access a BlockDevice
//!
//! A block device is a virtual or hardware device that can be read and written
//! to in blocks of a fixed size.

#![no_std]
#![allow(incomplete_features)] // for generic_const_exprs
#![feature(arbitrary_self_types, box_as_ptr, generic_const_exprs)]
#![warn(missing_docs)]

extern crate alloc;

use core::{error::Error, mem::size_of, ptr::NonNull};

use alloc::boxed::Box;
use log::error;
use shared::counts_required_for;

mod structs;
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
    ($size:literal: $block:ident,  $slice:ident) => {
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
            _pad: [u8; 4096 - core::mem::size_of::<T>()],
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
pub enum BlockDeviceOrMemError<BDError: Error + Send + Sync + 'static> {
    #[error("Block device error: {0}")]
    BlockDevice(#[from] BDError),
    #[error("Failed to allocate memory(RAM)")]
    Allocation,
}

/// A marker trait describing structs that can be constructed from a [super::Block].
///
/// This implies that it is safe to construct the struct from a byte slice read
/// from any block device.
pub trait BlockConstructable<const BLOCK_SIZE: usize> {}

/// A block device
///
/// this represents a virtual or hardware device that can be read and written to
/// in fixed size blocks.
// TODO why is data a NonNull and not just a ref?
pub trait BlockDevice {
    /// A gerneric error returned by the block device
    type BlockDeviceError: Error + Send + Sync + 'static;

    /// The block size of the device
    const BLOCK_SIZE: usize;

    /// A block aligned array of u8 of size [BLOCK_SIZE]
    type BlockSlice;

    /// Returns the number of blocks in the [BlockDevice]
    fn max_block_count(&self) -> Result<u64, Self::BlockDeviceError>;

    /// Read a block from the device
    ///
    /// The resulting slice will always be algined on block boundaries.
    fn read_block(&self, lba: LBA) -> Result<Box<Self::BlockSlice>, Self::BlockDeviceError>;

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
        data: NonNull<Self::BlockSlice>,
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
    fn read_block_atomic(&self, lba: LBA) -> Result<Box<Self::BlockSlice>, Self::BlockDeviceError>;

    /// Atomically write a block to the device
    ///
    /// If the device does not support atomic access, this should be implemented using a lock
    fn write_block_atomic(
        &mut self,
        lba: LBA,
        data: NonNull<Self::BlockSlice>,
    ) -> Result<(), Self::BlockDeviceError>;

    /// Atomically compare and exchange a block on the device
    ///
    /// If the device does not support atomic access, this should be implemented using a lock
    fn compare_exchange_block(
        &mut self,
        lba: LBA,
        current: NonNull<Self::BlockSlice>,
        new: NonNull<Self::BlockSlice>,
    ) -> Result<Result<(), Box<Self::BlockSlice>>, Self::BlockDeviceError>;

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
        assert!(data.len() as u64 <= group.count() * Self::BLOCK_SIZE as u64);
        self.write_blocks_contig(group.start, data)
    }

    /// Read `T` from [BlockDevice]
    ///
    /// TODO should this be marked as unsafe?
    ///
    /// # Safety
    ///
    /// We assume the pointer points to a `T` on the [BlockDevice] and that the
    /// data on the block device is not corrupted.
    /// There is no real way to guarantee this and this operation is always unsafe.
    fn read_pointer<T: BlockConstructable<{ Self::BLOCK_SIZE }>>(
        &self,
        ptr: DevicePointer<T>,
    ) -> Result<T, Self::BlockDeviceError> {
        if size_of::<T>() <= Self::BLOCK_SIZE {
            let data = self.read_block(ptr.lba)?;

            unsafe { Ok(((Box::as_ptr(&data)) as *const T).read()) }
        } else {
            let count = counts_required_for!(Self::BLOCK_SIZE, size_of::<T>()) as u64;
            let data = self.read_blocks_contig(ptr.lba, count)?;
            unsafe { Ok((data.as_ptr() as *const T).read()) }
        }
    }
}

#[cfg(any(feature = "test", test))]
#[allow(missing_docs)]
pub mod test {
    use alloc::boxed::Box;

    use thiserror::Error;

    use crate::BlockGroup;

    use super::{BlockDevice, BlockDeviceOrMemError, WriteData};

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

        type BlockSlice = TBlockSlice;

        fn max_block_count(&self) -> Result<u64, Self::BlockDeviceError> {
            Ok(0)
        }

        fn read_block(
            &self,
            _lba: crate::LBA,
        ) -> Result<Box<Self::BlockSlice>, Self::BlockDeviceError> {
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
            _data: core::ptr::NonNull<TBlockSlice>,
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
        ) -> Result<Box<TBlockSlice>, Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn write_block_atomic(
            &mut self,
            _lba: crate::LBA,
            _data: core::ptr::NonNull<TBlockSlice>,
        ) -> Result<(), Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn compare_exchange_block(
            &mut self,
            _lba: crate::LBA,
            _current: core::ptr::NonNull<TBlockSlice>,
            _new: core::ptr::NonNull<TBlockSlice>,
        ) -> Result<Result<(), Box<TBlockSlice>>, Self::BlockDeviceError> {
            Err(TestBlockDeviceError)
        }

        fn read_blocks<I>(
            &self,
            _blocks: I,
        ) -> Result<Box<[u8]>, BlockDeviceOrMemError<Self::BlockDeviceError>>
        where
            I: Iterator<Item = BlockGroup> + Clone,
        {
            Err(TestBlockDeviceError.into())
        }

        fn write_blocks<I>(
            &mut self,
            _blocks: I,
            _data: WriteData,
        ) -> Result<(), BlockDeviceOrMemError<Self::BlockDeviceError>>
        where
            I: Iterator<Item = BlockGroup> + Clone,
        {
            Err(TestBlockDeviceError.into())
        }
    }
}
