//! Additional traits and structs that depend on alloc

pub mod leak_allocator;
pub mod owned_slice;
pub mod reforbox;
mod single_arc;

use core::alloc::Allocator;
use core::alloc::Layout;

use alloc::alloc::Global;
use alloc::boxed::Box;
pub use single_arc::SingleArc;
pub use single_arc::Strong;
pub use single_arc::Weak;
pub use single_arc::WeakSingleArc;
use thiserror::Error;

/// A generic allocation error
#[allow(missing_docs)]
#[derive(Error, Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum AllocError {
    #[error("failed to allocate memory: {0}")]
    Alloc(#[from] core::alloc::AllocError),
    #[error("invalid layout {0}")]
    Layout(#[from] core::alloc::LayoutError),
}

/// Allocates a zeroed byte buffer
pub fn alloc_buffer(len: usize) -> Result<Box<[u8]>, AllocError> {
    let buffer = unsafe {
        // Safety: zeroed u8 are always init
        Box::try_new_zeroed_slice(len)?.assume_init()
    };
    Ok(buffer)
}

/// Allocates a zeroed byte buffer with the specified alignment
pub fn alloc_buffer_aligned(len: usize, align: usize) -> Result<Box<[u8]>, AllocError> {
    let layout = Layout::from_size_align(len, align)?;

    alloc_buffer_layout(layout)
}

/// Allocates a zeroed byte buffer with the specified layout
pub fn alloc_buffer_layout(layout: Layout) -> Result<Box<[u8]>, AllocError> {
    let raw = Global.allocate_zeroed(layout)?;

    unsafe {
        // Safety:
        // * raw points to allocated memory from the Global allocator
        // * layout matches the provided layout
        Ok(Box::from_raw(raw.as_ptr()))
    }
}

/// Allocates a zeroed byte buffer that can hold `T`
pub fn alloc_buffer_for<T: Sized>() -> Result<Box<[u8]>, AllocError> {
    let layout = Layout::new::<T>();

    alloc_buffer_layout(layout)
}

/// Allocates a zeroed byte buffer that can hold `count` `T`
pub fn alloc_buffer_for_multiple<T: Sized>(count: usize) -> Result<Box<[u8]>, AllocError> {
    let layout = Layout::array::<T>(count)?;

    alloc_buffer_layout(layout)
}
