#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::prelude::{LockCell, SpinLock};
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use core::{marker::PhantomData, ptr::null_mut};
use lazy_static::lazy_static;
use shared::rangeset::{Range, RangeSet};
use x86_64::{
    structures::paging::{
        FrameAllocator, FrameDeallocator, PageSize, PhysFrame, Size1GiB, Size2MiB, Size4KiB,
    },
    PhysAddr,
};

/// A wrapper around a RangeSet that allows allocation and deallocation of
/// Physical Frames of any size
#[derive(Clone, Copy)]
pub(super) struct PhysAllocator {
    memory_ranges: &'static SpinLock<RangeSet<{ Self::N }>>,
}

lazy_static! {
    static ref GLOBAL_PHYS_ALLOCATOR: SpinLock<RangeSet<{ PhysAllocator::N }>> =
        SpinLock::default();
    static ref KERNEL_FRAME_ALLOCATOR_4K: SpinLock<WasabiFrameAllocator<Size4KiB>> =
        SpinLock::new(WasabiFrameAllocator::new(PhysAllocator::get()));
    static ref KERNEL_FRAME_ALLOCATOR_2M: SpinLock<WasabiFrameAllocator<Size2MiB>> =
        SpinLock::new(WasabiFrameAllocator::new(PhysAllocator::get()));
    static ref KERNEL_FRAME_ALLOCATOR_1G: SpinLock<WasabiFrameAllocator<Size1GiB>> =
        SpinLock::new(WasabiFrameAllocator::new(PhysAllocator::get()));
}

impl PhysAllocator {
    pub const N: usize = 256;
    pub fn get() -> Self {
        Self {
            memory_ranges: &GLOBAL_PHYS_ALLOCATOR,
        }
    }

    pub(super) fn init(regions: &MemoryRegions) {
        let mut ranges = GLOBAL_PHYS_ALLOCATOR.lock();

        for region in regions
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
        {
            let range = Range {
                start: region.start,
                end: region.end - 1u64,
            };

            ranges.insert(range);
        }

        info!(
            "PhysAllocator::new() with {} usable regions mapped into {} ranges with a total of {} bytes",
            regions.iter()
                .filter(|r| r.kind == MemoryRegionKind::Usable)
                .count(),
            ranges.entries().len(),
            ranges.sum().expect("No physical memory found") // TODO: print deadlock
        );
    }

    pub fn alloc<S: PageSize>(&self) -> Option<PhysFrame<S>> {
        let size = S::SIZE;
        let align = S::SIZE;

        let alloc_start = self.memory_ranges.lock().allocate(size, align);
        alloc_start.map(|s| PhysAddr::new(s as u64)).map(|s| {
            PhysFrame::from_start_address(s).expect("range set should only return valid alignments")
        })
    }

    pub fn free<S: PageSize>(&self, frame: PhysFrame<S>) {
        self.memory_ranges.lock().insert(Range {
            start: frame.start_address().as_u64(),
            end: frame.start_address().as_u64() + frame.size() - 1u64,
        });
    }
}

#[repr(C)]
#[derive(Debug)]
struct UnusedFrame {
    size: u64,
    phys_addr: PhysAddr,
    next: *mut UnusedFrame,
}

pub struct WasabiFrameAllocator<S> {
    phys_alloc: PhysAllocator,
    first_unused_frame: *mut UnusedFrame,
    size: PhantomData<S>,
}

impl WasabiFrameAllocator<Size4KiB> {
    pub fn get_for_kernel() -> &'static SpinLock<Self> {
        &KERNEL_FRAME_ALLOCATOR_4K
    }
}

impl WasabiFrameAllocator<Size2MiB> {
    pub fn get_for_kernel() -> &'static SpinLock<Self> {
        &KERNEL_FRAME_ALLOCATOR_2M
    }
}

impl WasabiFrameAllocator<Size1GiB> {
    pub fn get_for_kernel() -> &'static SpinLock<Self> {
        &KERNEL_FRAME_ALLOCATOR_1G
    }
}

impl<S: PageSize> WasabiFrameAllocator<S> {
    pub(super) fn new(phys_allocator: PhysAllocator) -> Self {
        WasabiFrameAllocator {
            phys_alloc: phys_allocator,
            first_unused_frame: null_mut(),
            size: PhantomData::default(),
        }
    }

    pub const fn page_size() -> u64 {
        S::SIZE
    }

    pub const fn page_size_debug_name() -> &'static str {
        S::SIZE_AS_DEBUG_STR
    }

    pub fn alloc(&mut self) -> Option<PhysFrame<S>> {
        if self.first_unused_frame.is_null() {
            return self.phys_alloc.alloc();
        }

        todo!()
    }

    /// ## Safety
    ///
    /// The caller must ensure that the passed frame is unused.
    pub unsafe fn free(&mut self, frame: PhysFrame<S>) {
        // TODO store unused frames in free list

        self.phys_alloc.free(frame);
    }
}

// Safety: all frame allocations go through range-set which ensures that we
// only return unique frames
unsafe impl<S: PageSize> FrameAllocator<S> for WasabiFrameAllocator<S> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<S>> {
        self.alloc()
    }
}

impl<S: PageSize> FrameDeallocator<S> for WasabiFrameAllocator<S> {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<S>) {
        self.free(frame)
    }
}
