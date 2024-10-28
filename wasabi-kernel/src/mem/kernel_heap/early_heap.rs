//! A [Allocator] implementation for the early boot process.
//!
//! The heap provided here is extremely limited and should only be used
//! during the early boot process and the actual [kernel_heap] should be
//! initialized and used as early as possible.
//!
//! [kernel_heap]: super::kernel_heap

use core::{
    alloc::Layout,
    ptr::{addr_of, addr_of_mut},
    sync::atomic::{AtomicUsize, Ordering},
};

use shared::KiB;
use x86_64::align_up;

/// The size of the early boot kernel heap
///
/// This needs to be large enough to hold initial memory for the logger
/// created by the early boot startup, as well as all memory used
/// for logging before the normal kernel_heap is initialized.
///
/// This heap never frees memory and this value should be sized with that in mind.
pub const EARLY_HEAP_SIZE: usize = KiB!(4);

/// The memory used for the early boot heap
static mut EARLY_HEAP: Aligned4K<EARLY_HEAP_SIZE> = Aligned4K([0; EARLY_HEAP_SIZE]);

/// The offset of the next free byte
static EARLY_HEAP_NEXT_OFFSET: AtomicUsize = AtomicUsize::new(0);
/// The number of bytes allcoated
///
/// This differs from next_offset, because this does not count alignment
static EARLY_HEAP_ALLOCATED: AtomicUsize = AtomicUsize::new(0);
/// The number of bytes freed
static EARLY_HEAP_FREED: AtomicUsize = AtomicUsize::new(0);

#[repr(C, align(4096))]
struct Aligned4K<const N: usize>([u8; N]);

/// Allocate memory in the early boot heap
///
/// # Safety:
///
/// Must only be called during early boot process and with interrupts disabled
pub(super) unsafe fn alloc(layout: Layout) -> *mut u8 {
    let offset = EARLY_HEAP_NEXT_OFFSET.load(Ordering::Acquire);

    // Safety: we can align the offset, because the heap is page aligned and therefor any
    // alignement in the offset is also aligned on the heap
    let aligned_offset = align_up(offset as u64, layout.align() as u64) as usize;

    let end = aligned_offset + (layout.size() - 1);
    assert!(end <= EARLY_HEAP_SIZE, "EARLY HEAP OOM");

    EARLY_HEAP_NEXT_OFFSET.store(aligned_offset + layout.size(), Ordering::Release);
    EARLY_HEAP_ALLOCATED.fetch_add(layout.size(), Ordering::AcqRel);

    let base_ptr = addr_of_mut!(EARLY_HEAP) as *mut u8;
    let next_ptr = unsafe {
        // Safety: The ptr is within the ealry heap
        base_ptr.add(aligned_offset)
    };

    next_ptr
}

/// Frees memory in the early boot heap
pub(super) fn free(ptr: *mut u8, layout: Layout) {
    assert!(is_in_heap(ptr));
    EARLY_HEAP_FREED.fetch_add(layout.size(), Ordering::AcqRel);
}

/// Returns if a pointer points into the early heap
#[inline]
pub(super) fn is_in_heap(ptr: *const u8) -> bool {
    let start = addr_of!(EARLY_HEAP) as *const u8;
    let end = unsafe { start.add(EARLY_HEAP_SIZE - 1) };

    ptr >= start && ptr <= end
}

/// Return the number of bytes used in the early boot allocator
pub fn used() -> usize {
    EARLY_HEAP_NEXT_OFFSET.load(Ordering::Acquire)
}

/// Return the number of unused bytes in the early boot allocator
pub fn unused() -> usize {
    EARLY_HEAP_SIZE - used()
}

/// REturns the number of bytes freed in the early boot allocator
pub fn freed() -> usize {
    EARLY_HEAP_FREED.load(Ordering::Acquire)
}

/// The number of bytes allcoated oin the early boot allocator
///
/// This differs from [used], because this does not count alignment
pub fn allocated() -> usize {
    EARLY_HEAP_ALLOCATED.load(Ordering::Acquire)
}
