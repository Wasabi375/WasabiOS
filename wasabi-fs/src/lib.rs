#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(unused)] // TODO temp

use core::{
    mem::{size_of, transmute},
    num::{NonZeroU16, NonZeroU64},
    ops::{Add, AddAssign, Deref, DerefMut, Sub},
    ptr::NonNull,
    u64,
};

use nonmax::NonMaxU64;
use shared::math::IntoU64;
use static_assertions::const_assert;

extern crate alloc;

pub mod existing_fs_check;
pub mod fs;
pub mod fs_structs;
pub mod interface;
pub mod mem_structs;

/// Logical Block Address
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LBA(NonMaxU64);

impl TryFrom<u64> for LBA {
    type Error = <NonMaxU64 as TryFrom<u64>>::Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(LBA(value.try_into()?))
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

    pub const unsafe fn new_unchecked(addr: u64) -> Self {
        assert!(addr != u64::MAX);
        Self(unsafe { NonMaxU64::new_unchecked(addr) })
    }

    pub fn from_byte_offset(offset: u64) -> Option<LBA> {
        LBA::new(offset / BLOCK_SIZE as u64)
    }

    pub fn addr(self) -> NonMaxU64 {
        self.0
    }

    pub fn get(self) -> u64 {
        self.0.get()
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
    pub count_minus_one: u64,
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

/// Type alias for `[u8; BLOCK_SIZE]`
///
/// See [BLOCK_SIZE]
pub type BlockSlice = [u8; BLOCK_SIZE];

/// The size of any block used to store data on the disc
pub const BLOCK_SIZE: usize = 512;

/// Calculate the number of BLOCKs required for `bytes` memory.
///
/// # Example
///
/// ```no_run
/// # #[macro_use] extern crate wfs;
/// # fn main() {
/// # use static_assertions::const_assert_eq;
/// use wfs::blocks_required_for;
/// const_assert_eq!(1, blocks_required_for!( u8));
/// const_assert_eq!(1, blocks_required_for!( 512));
/// const_assert_eq!(1, blocks_required_for!( 8));
/// const_assert_eq!(2, blocks_required_for!( 1024));
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
pub const FS_VERSION: [u8; 4] = [0, 1, 0, 0];

/// Align `T` on block boundaries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(512))]
pub struct BlockAligned<T>(pub T);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ZST {}

/// Align `T` on block boundaries and ensure it is padded to fill the entire block
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(512))]
pub struct Block<T> {
    pub data: BlockAligned<T>,
    pad: BlockAligned<ZST>,
}

impl<T> Block<T> {
    pub fn new(data: T) -> Self {
        Self {
            data: BlockAligned(data),
            pad: BlockAligned(ZST {}),
        }
    }

    pub fn block_data(&self) -> NonNull<[u8; BLOCK_SIZE]> {
        assert!(size_of::<Self>() == 512);

        NonNull::from(self).cast()
    }

    pub fn multiblock_data(&self) -> NonNull<[u8]> {
        assert!(size_of::<Self>() % 512 == 0);

        NonNull::slice_from_raw_parts(NonNull::from(self).cast(), size_of::<Self>())
    }
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

#[cfg(test)]
mod test {
    use crate::{BlockGroup, LBA};

    #[test]
    fn test_block_from_start_end() {
        let start = LBA::new(1).unwrap();
        let end = LBA::new(10).unwrap();

        let group = BlockGroup::new(start, end);
        assert_eq!(start, group.start);
        assert_eq!(end, group.end());
    }
}
