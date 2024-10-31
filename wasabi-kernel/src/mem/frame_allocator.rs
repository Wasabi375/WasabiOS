//! Everything needed for PhysFrame allocation and managment

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use super::{MemError, Result};
use crate::prelude::{LockCell, UnwrapTicketLock};

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use shared::rangeset::{Range, RangeSet, RegionRequest};
use x86_64::{
    structures::paging::{
        frame::PhysFrameRangeInclusive, FrameAllocator as X86FrameAllocator, FrameDeallocator,
        PageSize, PhysFrame, Size1GiB, Size2MiB, Size4KiB,
    },
    PhysAddr,
};

#[cfg(feature = "mem-stats")]
use super::stats::PageFrameAllocStats;

/// The global kernel phys allocator. This is used by the frame allocators to create frames
// Safetey: this is initailized by [init] before it is used
static GLOBAL_PHYS_ALLOCATOR: UnwrapTicketLock<PhysAllocator> =
    unsafe { UnwrapTicketLock::new_uninit() };

/// A [FrameAllocator] for 4KiB pages
// Safetey: this is initailized by [init] before it is used
static KERNEL_FRAME_ALLOCATOR: UnwrapTicketLock<FrameAllocator> =
    unsafe { UnwrapTicketLock::new_uninit() };

/// initializes the frame allocators
pub fn init(regions: &MemoryRegions) {
    let mut ranges = RangeSet::<{ PhysAllocator::N }>::new();

    if log::log_enabled!(log::Level::Trace) {
        trace!("regions: ");
        for region in regions.iter() {
            trace!(
                "    {:?}: {:#X} - {:#X} | Size: {} bytes / {} page(s)",
                region.kind,
                region.start,
                region.end - 1,
                region.end - region.start,
                (region.end - region.start) / 4096
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
                "    {:#X} - {:#X} | Size: {} bytes / {} page(s)",
                entry.start,
                entry.end,
                entry.end - entry.start,
                (entry.end - entry.start) / 4096
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

    KERNEL_FRAME_ALLOCATOR
        .lock_uninit()
        .write(FrameAllocator::new(&GLOBAL_PHYS_ALLOCATOR));
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
pub struct FrameAllocator<'a> {
    /// the phys mem allocator used to allocate new frames, if we run out
    phys_alloc: &'a UnwrapTicketLock<PhysAllocator>,

    // TODO implement free list
    // /// a free list of unused frames
    // first_unused_frame: *mut UnusedFrame,
    /// memory to back gurad pages
    #[cfg(feature = "mem-backed-guard-page")]
    guard_frame: Option<PhysFrame<Size4KiB>>,

    #[cfg(feature = "mem-stats")]
    stats: PageFrameAllocStats,
}

impl FrameAllocator<'_> {
    /// Get the 4KiB frame allocator
    pub fn get_for_kernel() -> &'static UnwrapTicketLock<Self> {
        &KERNEL_FRAME_ALLOCATOR
    }
}

impl<'a> FrameAllocator<'a> {
    /// create a new allocator
    fn new(phys_allocator: &'a UnwrapTicketLock<PhysAllocator>) -> Self {
        FrameAllocator {
            phys_alloc: phys_allocator,
            // first_unused_frame: null_mut(),
            #[cfg(feature = "mem-backed-guard-page")]
            guard_frame: None,

            #[cfg(feature = "mem-stats")]
            stats: Default::default(),
        }
    }

    /// allocate a new frame
    pub fn alloc<S: PageSize>(&mut self) -> Result<PhysFrame<S>> {
        // if self.first_unused_frame.is_null() {
        return self
            .phys_alloc
            .lock()
            .alloc()
            .ok_or(MemError::OutOfMemory)
            .inspect(|_| {
                #[cfg(feature = "mem-stats")]
                self.stats.register_alloc::<S>(1);
            });
        // }

        // todo!()
    }

    /// allocate a new frame
    pub fn alloc_4k(&mut self) -> Result<PhysFrame<Size4KiB>> {
        self.alloc()
    }

    /// allocate a new frame
    pub fn alloc_2m(&mut self) -> Result<PhysFrame<Size2MiB>> {
        self.alloc()
    }

    /// allocate a new frame
    pub fn alloc_1g(&mut self) -> Result<PhysFrame<Size1GiB>> {
        self.alloc()
    }

    /// allocate a range of contigous frames
    pub fn alloc_range<S: PageSize>(&mut self, count: u64) -> Result<PhysFrameRangeInclusive<S>> {
        return self
            .phys_alloc
            .lock()
            .alloc_range(count)
            .ok_or(MemError::OutOfMemory)
            .inspect(|_| {
                #[cfg(feature = "mem-stats")]
                self.stats.register_alloc::<S>(count);
            });
    }

    /// Frees a phys frame
    ///
    /// ## Safety
    ///
    /// The caller must ensure that the passed frame is unused and not the
    /// [guard_frame]
    pub unsafe fn free<S: PageSize>(&mut self, frame: PhysFrame<S>) {
        // TODO store unused frames in free list

        self.phys_alloc.lock().free(frame);

        #[cfg(feature = "mem-stats")]
        self.stats.register_free::<S>(1);
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
    pub unsafe fn guard_frame(&mut self) -> Result<PhysFrame<Size4KiB>> {
        #[cfg(feature = "mem-backed-guard-page")]
        {
            if let Some(frame) = self.guard_frame {
                return Ok(frame);
            }
            let frame = self.alloc()?;
            self.guard_frame = Some(frame);
            Ok(frame)
        }
        #[cfg(not(feature = "mem-backed-guard-page"))]
        unsafe {
            Ok(PhysFrame::from_start_address_unchecked(PhysAddr::zero()))
        }
    }

    /// Access to [PageFrameAllocStats]
    #[cfg(feature = "mem-stats")]
    pub fn stats(&self) -> &PageFrameAllocStats {
        &self.stats
    }
}

// Safety: all frame allocations go through range-set which ensures that we
// only return unique frames
unsafe impl X86FrameAllocator<Size4KiB> for FrameAllocator<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        self.alloc().ok()
    }
}

impl FrameDeallocator<Size4KiB> for FrameAllocator<'_> {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        unsafe { self.free(frame) }
    }
}

// Safety: all frame allocations go through range-set which ensures that we
// only return unique frames
unsafe impl X86FrameAllocator<Size2MiB> for FrameAllocator<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size2MiB>> {
        self.alloc().ok()
    }
}

impl FrameDeallocator<Size2MiB> for FrameAllocator<'_> {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size2MiB>) {
        unsafe { self.free(frame) }
    }
}

// Safety: all frame allocations go through range-set which ensures that we
// only return unique frames
unsafe impl X86FrameAllocator<Size1GiB> for FrameAllocator<'_> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size1GiB>> {
        self.alloc().ok()
    }
}

impl FrameDeallocator<Size1GiB> for FrameAllocator<'_> {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size1GiB>) {
        unsafe { self.free(frame) }
    }
}
