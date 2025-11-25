//! A fake allocator that can't allocate and always leeks on free

use core::{alloc::Layout, ptr::NonNull};

use alloc::alloc::{AllocError, Allocator};
use log::{error, trace};

/// A fake allocator that can't allocate and always leeks on free
pub struct LeakAllocator {}

static STATIC: LeakAllocator = LeakAllocator {};

impl LeakAllocator {
    /// Provides access to a static [LeakAllocator]
    pub fn get() -> &'static Self {
        &STATIC
    }
}

unsafe impl Allocator for LeakAllocator {
    fn allocate(&self, _layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        error!("LeakAllocator can't allocate memory. Use a different allocator instaed");
        Err(AllocError)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        trace!("leak {ptr:p} with {layout:?}");
    }
}
