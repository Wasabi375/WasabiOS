pub mod base_allocator;
pub mod phys_allocator;

use thiserror::Error;

pub use x86_64::{PhysAddr, VirtAddr};

#[derive(Error, Debug)]
pub enum MemError {
    #[error("out of memory")]
    OutOfMemory,
    #[error("Zero size memory block")]
    BlockSizeZero,
    #[error("Null address is invalid")]
    NullAddress,
}
