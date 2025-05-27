#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![feature(
    allocator_api,
    arbitrary_self_types,
    assert_matches,
    box_as_ptr,
    debug_closure_helpers,
    negative_impls,
    try_reserve_kind
)]
#![allow(unused, dead_code)] // TODO temp
#![warn(unused_must_use, unused_mut, unused_labels)]

use core::{
    mem::{size_of, transmute},
    num::{NonZeroU16, NonZeroU64},
    ops::{Add, AddAssign, Deref, DerefMut, Sub, SubAssign},
    ptr::NonNull,
};

use nonmax::NonMaxU64;
use shared::{KiB, math::IntoU64};
use simple_endian::{LittleEndian, SpecificEndian};

extern crate alloc;

pub mod block_allocator;
pub mod existing_fs_check;
pub mod fs;
pub mod fs_structs;
pub mod interface;
pub mod mem_structs;
pub mod mem_tree;

/// Logical Block Address
///
/// Each [LBA] addresses a single [Block] on a [interface::BlockDevice].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LBA(LittleEndian<NonMaxU64>);

impl TryFrom<u64> for LBA {
    type Error = <NonMaxU64 as TryFrom<u64>>::Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(LBA(TryInto::<NonMaxU64>::try_into(value)?.into()))
    }
}

impl LBA {
    pub const fn new(addr: u64) -> Option<Self> {
        if addr == u64::MAX {
            None
        } else {
            unsafe { Some(Self::new_unchecked(addr)) }
        }
    }

    /// # Safety
    ///
    /// `addr` must not be `u64::MAX`
    pub const unsafe fn new_unchecked(addr: u64) -> Self {
        assert!(addr != u64::MAX);
        Self(LittleEndian::from_bits(unsafe {
            NonMaxU64::new_unchecked(addr.to_le())
        }))
    }

    pub fn from_byte_offset(offset: u64) -> Option<LBA> {
        LBA::new(offset / BLOCK_SIZE as u64)
    }

    pub fn to_byte_offset(self) -> u64 {
        self.get() * BLOCK_SIZE as u64
    }

    pub fn addr(self) -> NonMaxU64 {
        self.0.to_native()
    }

    pub fn get(self) -> u64 {
        self.0.to_native().get()
    }
}

impl Add<u64> for LBA {
    type Output = LBA;

    fn add(self, rhs: u64) -> Self::Output {
        unsafe { LBA::new_unchecked(self.get() + rhs) }
    }
}

impl AddAssign<u64> for LBA {
    fn add_assign(&mut self, rhs: u64) {
        *self = *self + rhs;
    }
}

impl Sub<LBA> for LBA {
    type Output = u64;

    fn sub(self, rhs: LBA) -> Self::Output {
        assert!(self >= rhs);

        self.get() - rhs.get()
    }
}

impl Sub<u64> for LBA {
    type Output = LBA;

    fn sub(self, rhs: u64) -> Self::Output {
        unsafe { LBA::new_unchecked(self.get() - rhs) }
    }
}

impl SubAssign<u64> for LBA {
    fn sub_assign(&mut self, rhs: u64) {
        *self = *self - rhs;
    }
}

impl PartialOrd for LBA {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LBA {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.get().cmp(&other.get())
    }
}

/// A group of contigous logical blocks
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockGroup {
    /// The fist block in the group
    pub start: LBA,
    /// The number of blocks in the group minus 1
    ///
    /// A group will always have at least 1 block. Therefor we can store
    /// this as `0`. 2 blocks will be represented as `1`, etc.
    // TODO why am I doing this. This is a u64
    pub count_minus_one: u64, // TODO LE
}

impl BlockGroup {
    pub fn new(start: LBA, end: LBA) -> Self {
        Self {
            start,
            count_minus_one: end - start,
        }
    }

    pub fn end(&self) -> LBA {
        self.start + self.count_minus_one
    }

    pub fn count(&self) -> u64 {
        self.count_minus_one + 1
    }

    pub fn contains(&self, lba: LBA) -> bool {
        self.start <= lba && lba <= self.end()
    }
}

/// Type alias for `BlockAligned<[u8; BLOCK_SIZE]>`
///
/// see [BlockAligned], [BLOCK_SIZE], [Block]
pub type BlockSlice = BlockAligned<[u8; BLOCK_SIZE]>;

/// The size of any block used to store data on the disc
///
/// See [BlockAligned], [Block], [BlockSlice], [blocks_required_for]
// NOTE: when changed, also change alignment of BlockAligned and Block.
pub const BLOCK_SIZE: usize = KiB!(4);

/// Calculate the number of BLOCKs required for `bytes` memory.
///
/// # Example
///
/// ```no_run
/// # #[macro_use] extern crate wfs;
/// # fn main() {
/// # use static_assertions::const_assert_eq;
/// # use shared::KiB;
/// use wfs::blocks_required_for;
/// const_assert_eq!(1, blocks_required_for!( type: u8));
/// const_assert_eq!(1, blocks_required_for!( 512));
/// const_assert_eq!(1, blocks_required_for!( 8));
/// const_assert_eq!(1, blocks_required_for!( KiB!(4)));
/// const_assert_eq!(2, blocks_required_for!( KiB!(8)));
/// const_assert_eq!(2, blocks_required_for!( KiB!(4) + 1));
/// const_assert_eq!(1, blocks_required_for!( size_of::<u32>() * 5));
/// # }
/// ```
#[macro_export]
macro_rules! blocks_required_for {
    (type: $type:ty) => {
        $crate::blocks_required_for!(core::mem::size_of::<$type>())
    };
    ($bytes:expr) => {
        shared::counts_required_for!($crate::BLOCK_SIZE as u64, $bytes as u64)
    };
}

/// The current version of the fs
///
/// See [fs_structs::MainHeader::version]
pub const FS_VERSION: [u8; 4] = [0, 1, 0, 3];

/// Align `T` on block boundaries
///
/// See [BLOCK_SIZE], [Block]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(4096))]
pub struct BlockAligned<T>(pub T);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Zst {}

/// Align `T` on block boundaries and ensure it is padded to fill the entire block
///
/// See [BLOCK_SIZE], [BlockAligned], [BlockSlice], [blocks_required_for]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(4096))]
pub struct Block<T> {
    _data: BlockAligned<T>,
    _pad: BlockAligned<Zst>,
}

impl<T> Block<T> {
    pub fn new(data: T) -> Self {
        Self {
            _data: BlockAligned(data),
            _pad: BlockAligned(Zst {}),
        }
    }

    pub fn into_inner(self) -> T {
        self._data.0
    }

    pub fn block_data(&self) -> NonNull<BlockSlice> {
        assert!(size_of::<Self>() == BLOCK_SIZE);

        NonNull::from(self).cast()
    }

    pub fn multiblock_data(&self) -> NonNull<[u8]> {
        assert!(size_of::<Self>() % BLOCK_SIZE == 0);

        NonNull::slice_from_raw_parts(NonNull::from(self).cast(), size_of::<Self>())
    }
}

impl<T> Deref for Block<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self._data.deref()
    }
}

impl<T> DerefMut for Block<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self._data.deref_mut()
    }
}

impl<T> Deref for BlockAligned<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for BlockAligned<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(feature = "test")]
testing::description::kernel_test_setup!();

#[cfg(test)]
mod test {
    use crate::{BlockGroup, LBA};

    #[test]
    /// ensure no off by 1 error when calculating `count_minus_one` field
    fn test_block_from_start_and_end() {
        let start = LBA::new(1).unwrap();
        let end = LBA::new(10).unwrap();

        let group = BlockGroup::new(start, end);
        assert_eq!(start, group.start);
        assert_eq!(end, group.end());
    }
}
