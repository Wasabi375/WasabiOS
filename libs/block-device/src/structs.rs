use core::{
    marker::PhantomData,
    mem::size_of,
    num::NonZeroU64,
    ops::{Add, AddAssign, Sub, SubAssign},
};

use nonmaxunsigned::{NonMaxU64, NonMaxU64Le};
use simple_endian::LittleEndian;
use static_assertions::const_assert;

/// Logical Block Address
///
/// Each [LBA] addresses a single [Block] on a [interface::BlockDevice].
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LBA(NonMaxU64Le);

impl TryFrom<u64> for LBA {
    type Error = <NonMaxU64Le as TryFrom<u64>>::Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(LBA(TryInto::<NonMaxU64Le>::try_into(value)?.into()))
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
        Self(unsafe { NonMaxU64Le::new_unchecked(addr) })
    }

    pub fn from_byte_offset<const BLOCK_SIZE: usize>(offset: u64) -> Option<LBA> {
        LBA::new(offset / BLOCK_SIZE as u64)
    }

    pub fn to_byte_offset<const BLOCK_SIZE: usize>(self) -> u64 {
        self.get() * BLOCK_SIZE as u64
    }

    pub fn addr(self) -> NonMaxU64 {
        self.0.to_native()
    }

    pub fn get(self) -> u64 {
        self.0.to_native().get()
    }

    pub const MAX: LBA = LBA(NonMaxU64Le::MAX);
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

    pub fn bytes(&self, block_size: usize) -> u64 {
        self.count() * block_size as u64
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

/// A pointer of type `T` into a [crate::interface::BlockDevice].
#[derive(Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct DevicePointer<T> {
    pub lba: LBA,
    _block_type: PhantomData<T>,
}
const_assert!(size_of::<DevicePointer<u8>>() == size_of::<LBA>());

impl<T> Clone for DevicePointer<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for DevicePointer<T> {}

impl<T> DevicePointer<T> {
    pub fn new(lba: LBA) -> Self {
        Self {
            lba,
            _block_type: PhantomData,
        }
    }
}

impl<T> core::ops::Receiver for DevicePointer<T> {
    type Target = T;
}
