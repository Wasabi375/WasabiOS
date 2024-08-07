//! Everything needed for PhysFrame allocation and managment

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::prelude::{LockCell, UnwrapTicketLock};
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use core::marker::PhantomData;
use shared::rangeset::{Range, RangeSet, RegionRequest};
use x86_64::{
    structures::paging::{
        frame::PhysFrameRangeInclusive, FrameAllocator, FrameDeallocator, PageSize, PhysFrame,
        Size1GiB, Size2MiB, Size4KiB,
    },
    PhysAddr,
};

/// a typealias for [UnwrapTicketLock] around a [PhysAllocator]
type LockedPhysAlloc = UnwrapTicketLock<PhysAllocator>;

/// The global kernel phys allocator. This is used by the frame allocators to create frames
// Safetey: this is initailized by [init] before it is used
static GLOBAL_PHYS_ALLOCATOR: LockedPhysAlloc = unsafe { UnwrapTicketLock::new_uninit() };
/// A [WasabiFrameAllocator] for 4KiB pages
// Safetey: this is initailized by [init] before it is used
static KERNEL_FRAME_ALLOCATOR_4K: UnwrapTicketLock<WasabiFrameAllocator<Size4KiB>> =
    unsafe { UnwrapTicketLock::new_uninit() };
/// A [WasabiFrameAllocator] for 2MiB pages
// Safetey: this is initailized by [init] before it is used
static KERNEL_FRAME_ALLOCATOR_2M: UnwrapTicketLock<WasabiFrameAllocator<Size2MiB>> =
    unsafe { UnwrapTicketLock::new_uninit() };
/// A [WasabiFrameAllocator] for 1GiB pages
// Safetey: this is initailized by [init] before it is used
static KERNEL_FRAME_ALLOCATOR_1G: UnwrapTicketLock<WasabiFrameAllocator<Size1GiB>> =
    unsafe { UnwrapTicketLock::new_uninit() };

/// initializes the frame allocators
pub fn init(regions: &MemoryRegions) {
    let mut ranges = RangeSet::<{ PhysAllocator::N }>::new();

    if log::log_enabled!(log::Level::Trace) {
        trace!("regions: ");
        for region in regions.iter() {
            trace!(
                "    {:?}: {:#X} - {:#X} | Size: {}",
                region.kind,
                region.start,
                region.end - 1,
                region.end - 1 - region.start
            );
        }
    }

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

    if log::log_enabled!(log::Level::Debug) {
        info!("Available Physical Ranges:");
        for entry in ranges.entries() {
            debug!(
                "    {:#X} - {:#X} | Size: {}",
                entry.start,
                entry.end,
                entry.end - entry.start
            );
        }
    }

    let mut allocator = PhysAllocator {
        memory_ranges: ranges,
    };

    reserve_phys_frames(&mut allocator);

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

fn reserve_phys_frames(allocator: &mut PhysAllocator) {
    crate::apic::ap_startup::reserve_phys_frames(allocator);
}

/// A wrapper around a RangeSet that allows allocation and deallocation of
/// Physical Frames of any size
#[derive(Clone, Copy)]
pub struct PhysAllocator {
    memory_ranges: RangeSet<{ Self::N }>,
}

impl PhysAllocator {
    /// Number of ranges in the internal [RangeSet]
    pub const N: usize = 256;

    /// allocate a  new frame
    pub fn alloc<S: PageSize>(&mut self) -> Option<PhysFrame<S>> {
        let size = S::SIZE;
        let align = S::SIZE;

        let alloc_start = self.memory_ranges.allocate(size, align);
        alloc_start.map(|s| PhysAddr::new(s as u64)).map(|s| {
            PhysFrame::from_start_address(s).expect("range set should only return valid alignments")
        })
    }

    /// allocate a new frame within the given range
    pub fn alloc_in_range<S: PageSize>(&mut self, range: RegionRequest) -> Option<PhysFrame<S>> {
        let size = S::SIZE;
        let align = S::SIZE;

        let alloc_start = self.memory_ranges.allocate_prefer(size, align, range);
        alloc_start.map(|s| PhysAddr::new(s as u64)).map(|s| {
            PhysFrame::from_start_address(s).expect("range set should only return valid alignments")
        })
    }

    /// allocate a range of contigous frames
    pub fn alloc_range<S: PageSize>(&mut self, count: u64) -> Option<PhysFrameRangeInclusive<S>> {
        assert_ne!(count, 0);

        let size = S::SIZE * count;
        let align = S::SIZE;

        let alloc_start = self.memory_ranges.allocate(size, align);
        alloc_start
            .map(|s| PhysAddr::new(s as u64))
            .map(|s| {
                PhysFrame::from_start_address(s)
                    .expect("range set should only return valid alignments")
            })
            .map(|start_frame| PhysFrameRangeInclusive {
                start: start_frame,
                end: start_frame + (count - 1),
            })
    }

    /// free a frame
    pub fn free<S: PageSize>(&mut self, frame: PhysFrame<S>) {
        self.memory_ranges.insert(Range {
            start: frame.start_address().as_u64(),
            end: frame.start_address().as_u64() + frame.size() - 1u64,
        });
    }

    /// free all frames
    pub fn free_all<S: PageSize, I: Iterator<Item = PhysFrame<S>>>(&mut self, frames: I) {
        for frame in frames {
            self.free(frame);
        }
    }
}

/// a linked list of unused phys frames
#[repr(C)]
#[derive(Debug)]
#[allow(dead_code)] // TODO
struct UnusedFrame {
    size: u64,
    phys_addr: PhysAddr,
    next: *mut UnusedFrame,
}

/// A PhysFrame Allocator
pub struct WasabiFrameAllocator<'a, S> {
    /// the phys mem allocator used to allocate new frames, if we run out
    phys_alloc: &'a LockedPhysAlloc,

    /// the size of frames this allocator uses
    size: PhantomData<S>,
    // TODO implement free list
    // /// a free list of unused frames
    // first_unused_frame: *mut UnusedFrame,
    /// memory to back gurad pages
    #[cfg(feature = "mem-backed-guard-page")]
    guard_frame: Option<PhysFrame<S>>,
}

impl WasabiFrameAllocator<'_, Size4KiB> {
    /// Get the 4KiB frame allocator
    pub fn get_for_kernel() -> &'static UnwrapTicketLock<Self> {
        &KERNEL_FRAME_ALLOCATOR_4K
    }
}

impl WasabiFrameAllocator<'_, Size2MiB> {
    /// Get the 2MiB frame allocator
    pub fn get_for_kernel() -> &'static UnwrapTicketLock<Self> {
        &KERNEL_FRAME_ALLOCATOR_2M
    }
}

impl WasabiFrameAllocator<'_, Size1GiB> {
    /// Get the 1GiB frame allocator
    pub fn get_for_kernel() -> &'static UnwrapTicketLock<Self> {
        &KERNEL_FRAME_ALLOCATOR_1G
    }
}

impl<'a, S: PageSize> WasabiFrameAllocator<'a, S> {
    /// create a new allocator
    fn new(phys_allocator: &'a LockedPhysAlloc) -> Self {
        WasabiFrameAllocator {
            phys_alloc: phys_allocator,
            // first_unused_frame: null_mut(),
            size: PhantomData::default(),
        }
    }

    /// the page size in bytes
    pub const fn page_size() -> u64 {
        S::SIZE
    }

    /// the debug name of the page size
    pub const fn page_size_debug_name() -> &'static str {
        S::DEBUG_STR
    }

    /// allocate a new frame
    // TODO return Result<_, MemError>
    pub fn alloc(&mut self) -> Option<PhysFrame<S>> {
        // if self.first_unused_frame.is_null() {
        return self.phys_alloc.lock().alloc();
        // }

        // todo!()
    }

    /// allocate a range of contigous frames
    pub fn alloc_range(&mut self, count: u64) -> Option<PhysFrameRangeInclusive<S>> {
        return self.phys_alloc.lock().alloc_range(count);
    }

    /// Frees a phys frame
    ///
    /// ## Safety
    ///
    /// The caller must ensure that the passed frame is unused and not the
    /// [guard_frame]
    pub unsafe fn free(&mut self, frame: PhysFrame<S>) {
        // TODO store unused frames in free list

        self.phys_alloc.lock().free(frame);
    }

    /// returns the [PhysFrame] used for guard pages.
    ///
    /// if "mem-backed-guard-frame" feature is active, this will be a
    /// cached allocated frame. Otherwise it will be the frame
    /// starting at address 0
    ///
    /// # Safety:
    ///
    /// The Frame should only be used for guard pages and is not guaranteed to be a valid
    /// phyisical address
    pub unsafe fn guard_frame(&mut self) -> Option<PhysFrame<S>> {
        #[cfg(feature = "mem-backed-guard-page")]
        {
            if let Some(frame) = self.guard_frame {
                return Some(frame);
            }
            let frame = self.alloc();
            self.guard_frame = frame;
            frame
        }
        #[cfg(not(feature = "mem-backed-guard-page"))]
        unsafe {
            Some(PhysFrame::from_start_address_unchecked(PhysAddr::zero()))
        }
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
        unsafe { self.free(frame) }
    }
}
