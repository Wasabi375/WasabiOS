#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::prelude::{LockCell, UnwrapSpinLock};
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use core::{marker::PhantomData, ptr::null_mut};
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
struct PhysAllocator {
    memory_ranges: RangeSet<{ Self::N }>,
}

type LockedPhysAlloc = UnwrapSpinLock<PhysAllocator>;

static GLOBAL_PHYS_ALLOCATOR: LockedPhysAlloc = unsafe { UnwrapSpinLock::new_uninit() };
static KERNEL_FRAME_ALLOCATOR_4K: UnwrapSpinLock<WasabiFrameAllocator<Size4KiB>> =
    unsafe { UnwrapSpinLock::new_uninit() };
static KERNEL_FRAME_ALLOCATOR_2M: UnwrapSpinLock<WasabiFrameAllocator<Size2MiB>> =
    unsafe { UnwrapSpinLock::new_uninit() };
static KERNEL_FRAME_ALLOCATOR_1G: UnwrapSpinLock<WasabiFrameAllocator<Size1GiB>> =
    unsafe { UnwrapSpinLock::new_uninit() };

pub fn init(regions: &MemoryRegions) {
    let mut ranges = RangeSet::<{ PhysAllocator::N }>::new();

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

    let allocator = PhysAllocator {
        memory_ranges: ranges,
    };
    GLOBAL_PHYS_ALLOCATOR.lock_uninit().write(allocator);

    info!(
        "PhysAllocator::new() with {} usable regions mapped into {} ranges \
             with a total of {} bytes",
        regions
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
            .count(),
        ranges.entries().len(),
        ranges.sum().expect("No physical memory found")
    );

    KERNEL_FRAME_ALLOCATOR_4K
        .lock_uninit()
        .write(WasabiFrameAllocator::new(&GLOBAL_PHYS_ALLOCATOR));
    KERNEL_FRAME_ALLOCATOR_2M
        .lock_uninit()
        .write(WasabiFrameAllocator::new(&GLOBAL_PHYS_ALLOCATOR));
    KERNEL_FRAME_ALLOCATOR_1G
        .lock_uninit()
        .write(WasabiFrameAllocator::new(&GLOBAL_PHYS_ALLOCATOR));
}

impl PhysAllocator {
    pub const N: usize = 256;

    pub fn alloc<S: PageSize>(&mut self) -> Option<PhysFrame<S>> {
        let size = S::SIZE;
        let align = S::SIZE;

        let alloc_start = self.memory_ranges.allocate(size, align);
        alloc_start.map(|s| PhysAddr::new(s as u64)).map(|s| {
            PhysFrame::from_start_address(s).expect("range set should only return valid alignments")
        })
    }

    pub fn free<S: PageSize>(&mut self, frame: PhysFrame<S>) {
        self.memory_ranges.insert(Range {
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

pub struct WasabiFrameAllocator<'a, S> {
    phys_alloc: &'a LockedPhysAlloc,
    first_unused_frame: *mut UnusedFrame,
    size: PhantomData<S>,
}

impl WasabiFrameAllocator<'_, Size4KiB> {
    pub fn get_for_kernel() -> &'static UnwrapSpinLock<Self> {
        &KERNEL_FRAME_ALLOCATOR_4K
    }
}

impl WasabiFrameAllocator<'_, Size2MiB> {
    pub fn get_for_kernel() -> &'static UnwrapSpinLock<Self> {
        &KERNEL_FRAME_ALLOCATOR_2M
    }
}

impl WasabiFrameAllocator<'_, Size1GiB> {
    pub fn get_for_kernel() -> &'static UnwrapSpinLock<Self> {
        &KERNEL_FRAME_ALLOCATOR_1G
    }
}

impl<'a, S: PageSize> WasabiFrameAllocator<'a, S> {
    fn new(phys_allocator: &'a LockedPhysAlloc) -> Self {
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
            return self.phys_alloc.lock().alloc();
        }

        todo!()
    }

    /// ## Safety
    ///
    /// The caller must ensure that the passed frame is unused.
    pub unsafe fn free(&mut self, frame: PhysFrame<S>) {
        // TODO store unused frames in free list

        self.phys_alloc.lock().free(frame);
    }
}

// Safety: all frame allocations go through range-set which ensures that we
// only return unique frames
unsafe impl<S: PageSize> FrameAllocator<S> for WasabiFrameAllocator<'_, S> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<S>> {
        self.alloc()
    }
}

impl<S: PageSize> FrameDeallocator<S> for WasabiFrameAllocator<'_, S> {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<S>) {
        self.free(frame)
    }
}
