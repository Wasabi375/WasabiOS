#![cfg_attr(not(test), no_std)]
#![allow(incomplete_features)] // for generic_const_exprs
#![feature(
    allocator_api,
    arbitrary_self_types,
    assert_matches,
    box_as_ptr,
    debug_closure_helpers,
    generic_const_exprs,
    negative_impls,
    try_reserve_kind,
    try_with_capacity,
    vec_push_within_capacity
)]

// FIXME #![warn(missing_docs)]

use block_device::block_size_types;
use shared::KiB;

extern crate alloc;

pub mod block_allocator;
pub mod existing_fs_check;
pub mod fs;
pub mod fs_structs;
pub mod mem_structs;
pub mod mem_tree;

/// The size of any block used to store data on the disc
///
/// See [BlockAligned], [Block], [BlockSlice], [blocks_required_for]
// NOTE: when changed, also change alignment of BlockAligned and Block.
pub const BLOCK_SIZE: usize = KiB!(4);
block_size_types!(4096: Block, BlockArray);

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
/// The bytes corespond to major, minor, patch, alpha-status
///
/// ### Alpha-status
///
/// - 255: released
/// - 0..254: dev, incremented on breaking change during dev.
///
/// See [fs_structs::MainHeader::version]
pub const FS_VERSION: [u8; 4] = [0, 1, 0, 255];

#[cfg(feature = "test")]
testing::description::kernel_test_setup!();
