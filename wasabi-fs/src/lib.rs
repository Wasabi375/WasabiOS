#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]
#![feature(
    allocator_api,
    arbitrary_self_types,
    assert_matches,
    box_as_ptr,
    debug_closure_helpers,
    negative_impls,
    try_reserve_kind,
    try_with_capacity,
    vec_push_within_capacity
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
use static_assertions::const_assert_eq;

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
    #[track_caller]
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

    pub const MAX: LBA = unsafe { Self::new_unchecked(u64::MAX - 1) };
}

impl Add<u64> for LBA {
    type Output = LBA;

    #[track_caller]
    fn add(self, rhs: u64) -> Self::Output {
        assert_ne!(self.get() + rhs, u64::MAX);
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
        // Saftey: subtraction can never result in max
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
    /// The number of blocks in the group
    pub count: LittleEndian<NonZeroU64>,
}

impl BlockGroup {
    pub fn new(start: LBA, end: LBA) -> Self {
        Self {
            start,
            count: NonZeroU64::new(end.get() + 1 - start.get())
                .expect("end should be greater than start")
                .into(),
        }
    }

    pub fn with_count(start: LBA, count: NonZeroU64) -> Self {
        Self {
            start,
            count: count.into(),
        }
    }

    pub fn end(&self) -> LBA {
        self.start + self.count() - 1
    }

    pub fn count(&self) -> u64 {
        self.count.to_native().get()
    }

    pub fn bytes(&self) -> u64 {
        self.count() * BLOCK_SIZE as u64
    }

    pub fn contains(&self, lba: LBA) -> bool {
        self.start <= lba && lba <= self.end()
    }

    pub fn subgroup(&self, block_offset: u64) -> Self {
        assert!(self.count() > block_offset);
        BlockGroup::new(self.start + block_offset, self.end())
    }

    pub fn remove_end(&self, blocks_to_remove: u64) -> Self {
        assert!(self.count() > blocks_to_remove);
        BlockGroup::new(self.start, self.end() - blocks_to_remove)
    }

    pub fn shorten(&self, new_length: u64) -> Self {
        assert!(new_length <= self.count());
        self.remove_end(self.count() - new_length)
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
    pub data: BlockAligned<T>,
    _pad: BlockAligned<Zst>,
}
const_assert_eq!(size_of::<Block<[u8; BLOCK_SIZE]>>(), BLOCK_SIZE);

impl<T> Block<T> {
    pub const fn new(data: T) -> Self {
        Self {
            data: BlockAligned(data),
            _pad: BlockAligned(Zst {}),
        }
    }

    pub fn into_inner(self) -> T {
        self.data.0
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

impl Block<BlockSlice> {
    /// A zero filled block
    pub const ZERO: Block<BlockSlice> = Block::new(BlockAligned([0; BLOCK_SIZE]));
}

impl<T> Deref for Block<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.data.deref()
    }
}

impl<T> DerefMut for Block<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data.deref_mut()
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
    /// ensure no off by 1 error when calculating `count` field
    fn test_block_from_start_and_end() {
        let start = LBA::new(1).unwrap();
        let end = LBA::new(10).unwrap();

        let group = BlockGroup::new(start, end);
        assert_eq!(start, group.start);
        assert_eq!(end, group.end());
        assert_eq!(10, group.count());
    }
}
