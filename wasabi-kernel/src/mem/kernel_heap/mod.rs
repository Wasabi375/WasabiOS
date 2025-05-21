//! provides [GlobalAlloc] for the kernel
//!

pub mod early_heap;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

#[cfg(feature = "mem-stats")]
use super::stats::HeapStats;

#[cfg(feature = "freeze-heap")]
use crate::prelude::ReadWriteCell;
#[cfg(feature = "freeze-heap")]
use shared::sync::lockcell::RWLockCell;

use super::{ptr::UntypedPtr, structs::Pages};
use crate::{
    mem::{
        frame_allocator::FrameAllocator, page_allocator::PageAllocator, page_table::PageTable,
        MemError, Result,
    },
    prelude::{LockCell, TicketLock, UnwrapTicketLock},
};

use core::{
    alloc::{GlobalAlloc, Layout},
    mem::{align_of, size_of, MaybeUninit},
    ops::DerefMut,
    ptr::{null_mut, NonNull},
    sync::atomic::{AtomicBool, Ordering},
};
use linked_list_allocator::Heap as LinkedHeap;
use shared::KiB;
use x86_64::structures::paging::{PageSize, PageTableFlags, Size4KiB};

/// the size of the kernel heap in bytes
pub const KERNEL_HEAP_SIZE: usize =
    KernelHeapPageSize::SIZE as usize * KERNEL_HEAP_PAGE_COUNT as usize;

/// the size of a single page in the kernel heap
type KernelHeapPageSize = Size4KiB;

/// number of memory pages used by the kernel heap
const KERNEL_HEAP_PAGE_COUNT: u64 = 250;

/// the [KernelHeap]
// Safety: initialized by [init] before it we use allocated types
static KERNEL_HEAP: UnwrapTicketLock<KernelHeap> =
    unsafe { UnwrapTicketLock::new_non_preemtable_uninit() };

/// The global allocator
#[global_allocator]
static GLOBAL_ALLOCATOR: KernelHeapGlobalAllocator = KernelHeapGlobalAllocator::new();

/// true if the kernel heap is initiaized.
static KERNEL_HEAP_INIT: AtomicBool = AtomicBool::new(false);

/// initializes the kernel heap
pub fn init() {
    info!(
        "init kernel heap: {} bytes, {} pages",
        KERNEL_HEAP_SIZE, KERNEL_HEAP_PAGE_COUNT
    );

    let pages = PageAllocator::get_for_kernel()
        .lock()
        .also(|_| {
            trace!("page alloc lock aquired");
        })
        .allocate_pages::<KernelHeapPageSize>(KERNEL_HEAP_PAGE_COUNT)
        .expect("Out of pages setting up kernel heap");

    {
        let mut page_table = PageTable::get_for_kernel().lock();
        trace!("page table lock aquired");

        let mut frame_allocator = FrameAllocator::get_for_kernel().lock();
        trace!("frame alloc lock aquired");
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;

        for page in pages.iter() {
            let frame = frame_allocator
                .alloc()
                .expect("Out of memory setting up kernel heap");
            unsafe {
                // Safety: we are mapping new unused pages to new unused phys frames
                match page_table.map_kernel(page, frame, flags, frame_allocator.deref_mut()) {
                    Ok(flusher) => flusher.flush(),
                    Err(e) => panic!("Failed to map page {page:?} to frame {frame:?}: {e:?}"),
                }
            }
        }
    }

    let mut heap_guard = KERNEL_HEAP.lock_uninit();
    let heap = unsafe {
        // Safety: we call init right after we move this to static storage
        heap_guard.write(KernelHeap::new(pages.into()))
    };

    // Safety: KERNEL_HEAP is a static
    if let Err(e) = unsafe { heap.init() } {
        panic!("KernelHeap::new(): {e:?}")
    };
    drop(heap_guard);

    KERNEL_HEAP_INIT.store(true, Ordering::SeqCst);
    debug!(
        "early kernel heap memory stats: {} used of {} total. {}({}%) freed",
        early_heap::used(),
        early_heap::EARLY_HEAP_SIZE,
        early_heap::freed(),
        early_heap::freed() as f32 / early_heap::allocated() as f32
    );
    if early_heap::used() > 0 {
        warn!("Early heap used. Ensure this was intentional and update this check");
    }

    #[cfg(feature = "mem-stats")]
    KernelHeap::init_stats(KernelHeap::get(), HeapStats::default());

    trace!("kernel init done");
}

/// [GlobalAlloc] implementation
struct KernelHeapGlobalAllocator {
    #[cfg(feature = "freeze-heap")]
    freeze_heaps: ReadWriteCell<GlobalHeapFreezeHeaps>,
}

impl KernelHeapGlobalAllocator {
    const fn new() -> Self {
        Self {
            #[cfg(feature = "freeze-heap")]
            freeze_heaps: ReadWriteCell::new_non_preemtable(GlobalHeapFreezeHeaps::new()),
        }
    }
}

#[cfg(feature = "freeze-heap")]
struct GlobalHeapFreezeHeaps {
    /// number of extra heaps that are in use
    valid_count: usize,

    /// the extra kernel heaps used by the freeze-heap feature
    ///
    /// The last valid heap as indicated by [Self::valid_extra_heap_count]
    /// is the active heap used by all allocations. If there is no extra heap
    /// the normal kernel heap is used.
    ///
    /// Dealocations are executed by the heap that did the allocation.
    extra_heaps: [MaybeUninit<TicketLock<KernelHeap>>; Self::MAX_HEAPS],
}

#[cfg(feature = "freeze-heap")]
impl GlobalHeapFreezeHeaps {
    const fn new() -> Self {
        Self {
            valid_count: 0,
            extra_heaps: [const { MaybeUninit::uninit() }; Self::MAX_HEAPS],
        }
    }

    const MAX_HEAPS: usize = 8;
}

unsafe impl GlobalAlloc for KernelHeapGlobalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !KERNEL_HEAP_INIT.load(Ordering::Acquire) {
            unsafe {
                // Safety: only called while main heap is not initialized
                // and the main heap is initialized early during boot process
                return early_heap::alloc(layout);
            }
        }

        // only log if we are using the main heap, logging might not be initialized
        trace!(target: "GlobalAlloc", "allocate {layout:?}");

        #[cfg(not(feature = "freeze-heap"))]
        let result = KernelHeap::get().alloc(layout);
        #[cfg(feature = "freeze-heap")]
        let result = {
            let freeze_heaps = self.freeze_heaps.read();
            if freeze_heaps.valid_count > 0 {
                let active_heap_idx = freeze_heaps.valid_count - 1;
                unsafe {
                    // Safety: heap is active based on valid_extra_heap_count
                    freeze_heaps.extra_heaps[active_heap_idx].assume_init_ref()
                }
                .lock()
                .alloc(layout)
            } else {
                KernelHeap::get().alloc(layout)
            }
        };

        match result {
            Ok(mem) => unsafe {
                trace!(target: "GlobalAlloc", "allocating at {:p}", mem);
                // Safety: [KernelHeap::alloc] returns a valid pointer
                // and we still have unique access to it
                mem.as_mut()
            },
            Err(err) => {
                error!(target: "GlobalAlloc", "Kernel Heap allocation failed for layout {layout:?}: {err:?}");
                null_mut()
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if !KERNEL_HEAP_INIT.load(Ordering::Acquire) {
            // main heap is not init so all frees are within the early_heap
            early_heap::free(ptr, layout);
            return;
        }

        // only log if we are using the main heap, logging might not be initialized
        trace!(target: "GlobalAlloc", "free {ptr:p} - {layout:?}");

        if early_heap::is_in_heap(ptr) {
            early_heap::free(ptr, layout);
        }

        let ptr = unsafe {
            // Safety: see safety guarantees of [GlobalAlloc]
            UntypedPtr::new_from_raw(ptr)
        };
        let Some(non_null) = ptr else {
            error!(target: "GlobalAlloc", "Tried to free null pointer with layout {layout:?}");
            return;
        };

        #[cfg(not(feature = "freeze-heap"))]
        {
            // Safety: see safety guarantees of [GlobalAlloc]
            match unsafe { KernelHeap::get().free(non_null, layout) } {
                Ok(_) => {}
                Err(err) => {
                    error!(target: "GlobalAlloc", "Failed to free {non_null:p} with layout {layout:?}: {err:?}");
                }
            }
        }
        #[cfg(feature = "freeze-heap")]
        {
            let freeze_heaps = self.freeze_heaps.read();
            let extras = if freeze_heaps.valid_count > 0 {
                unsafe {
                    // Safety: the first `valid_extra_heap_count` heaps are initialized
                    freeze_heaps.extra_heaps[0..freeze_heaps.valid_count].assume_init_ref()
                }
            } else {
                &[]
            };

            for allocator in extras {
                let mut allocator = allocator.lock();
                // Safety: see safety guarantees of [GlobalAlloc]
                match unsafe { allocator.free(non_null, layout) } {
                    Ok(_) => {
                        return;
                    }
                    Err(MemError::PtrNotAllocated(_)) => continue,
                    Err(err) => {
                        error!(target: "GlobalAlloc", "Failed to free {non_null:p} with layout {layout:?}: {err:?}");
                        return;
                    }
                }
            }
            // Safety: see safety guarantees of [GlobalAlloc]
            match unsafe { KernelHeap::get().free(non_null, layout) } {
                Ok(_) => {}
                Err(err) => {
                    error!(target: "GlobalAlloc", "Failed to free {non_null:p} with layout {layout:?}: {err:?}");
                }
            }
        }
    }
}

/// Locks the currently active heap and creates a new heap that is used
/// for any new allocations.
///
/// [try_unfreeze_global_heap] can be used to undo this operation.
#[cfg(feature = "freeze-heap")]
pub fn freeze_global_heap() -> Result<()> {
    // must be allocated before we take the freeze_heaps write lock. Otherwise
    // this leads to a deadlock when we try to allocate the histogram for the stats object
    let empty_stats = HeapStats::default();

    let mut freeze_heaps = GLOBAL_ALLOCATOR.freeze_heaps.write();

    if freeze_heaps.valid_count == freeze_heaps.extra_heaps.len() {
        return Err(MemError::MaxHeapCountReached);
    }

    let pages = PageAllocator::get_for_kernel()
        .lock()
        .allocate_pages::<KernelHeapPageSize>(KERNEL_HEAP_PAGE_COUNT)?;

    {
        let mut page_table = PageTable::get_for_kernel().lock();

        let mut frame_allocator = FrameAllocator::get_for_kernel().lock();
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;

        for page in pages.iter() {
            let frame = frame_allocator.alloc()?;
            unsafe {
                // Safety: we are mapping new unused pages to new unused phys frames
                page_table
                    .map_kernel(page, frame, flags, frame_allocator.deref_mut())?
                    .flush();
            }
        }
    }

    let new_heap = unsafe {
        // Safety: pages are mapped and we call init right after creation
        TicketLock::new_non_preemtable(KernelHeap::new(pages))
    };
    let heap_id = freeze_heaps.valid_count;

    let new_heap = freeze_heaps.extra_heaps[heap_id].write(new_heap);
    unsafe {
        // Safety: new_heap lives in the global allocator which is a static
        new_heap.lock().init()?;
    }
    KernelHeap::init_stats(new_heap, empty_stats);

    freeze_heaps.valid_count += 1;

    log::debug!("Kernel Heap frozen");

    Ok(())
}

/// Tries to free the active freeze heap.
///
/// A freeze heap can be allocated by calling [freeze_global_heap].
/// This fails if there is no freeze heap or if there are any open allocations
/// in the current freeze heap.
#[cfg(feature = "freeze-heap")]
pub fn try_unfreeze_global_heap() -> Result<()> {
    let mut freeze_heaps = GLOBAL_ALLOCATOR.freeze_heaps.write();

    if freeze_heaps.valid_count == 0 {
        return Err(MemError::NoFreezeHeapExists);
    }

    let to_free_id = freeze_heaps.valid_count - 1;

    let heap = unsafe {
        // Safety: not yet freed so this should be valid
        freeze_heaps.extra_heaps[to_free_id].assume_init_read()
    };
    let heap = heap.lock();

    if heap.stats().used > 0 {
        return Err(MemError::HeapNotFullyFreed);
    }
    freeze_heaps.valid_count -= 1;

    drop(freeze_heaps);

    // lock and heap must be dropped after the write lock is released. Otherwise
    // dropping the heap might lead to a deadlock as it tries to access the GlobalAllocator
    // when trying to drop the Histrogram for the stats object.
    // TODO todo_warn!("Free pages and frames used by freeze heap. Drop for KernelHeap?");
    drop(heap);

    Ok(())
}

/// The current number of freeze heaps.
///
/// See [freeze_global_heap]
#[cfg(feature = "freeze-heap")]
pub fn freeze_heap_count() -> usize {
    GLOBAL_ALLOCATOR.freeze_heaps.read().valid_count
}

/// The maximum number of freeze heaps.
///
/// See [freeze_global_heap]
#[cfg(feature = "freeze-heap")]
pub fn max_freeze_heap_count() -> usize {
    GlobalHeapFreezeHeaps::MAX_HEAPS
}

/// the sizes of the [SlabAllocator]s used by the kernel heap
const SLAB_ALLOCATOR_SIZES_BYTES: &'static [usize] = &[4, 64, 256];

/// block size for the [SlabAllocator]
const SLAB_BLOCK_SIZE: usize = KiB!(1);

/// The allocator trait used by the kernel
pub trait Allocator {
    /// allocate [Layout] and return either a non null [u8] pointer or [MemError]
    fn alloc(&self, layout: Layout) -> Result<UntypedPtr>;

    /// free a pointer with a given [Layout]. This can fail with [MemError],
    /// e.g. when the `ptr` was not allocated by this [Allocator]
    ///
    /// # Safety
    ///
    /// the caller must ensure that the ptr and layout match
    /// and that the ptr was allocated by this [Allocator]
    unsafe fn free(&self, ptr: UntypedPtr, layout: Layout) -> Result<()>;
}

/// Same allocator trait as [Allocator] but requires mut access to `self`
pub trait MutAllocator {
    /// allocate [Layout] and return either a non null [u8] pointer or [MemError]
    fn alloc(&mut self, layout: Layout) -> Result<UntypedPtr>;

    /// free a pointer with a given [Layout]. This can fail with [MemError],
    /// e.g. when the `ptr` was not allocated by this [Allocator]
    ///
    /// # Safety
    ///
    /// the caller must ensure that the ptr and layout match
    /// and that the ptr was allocated by this [MutAllocator]
    unsafe fn free(&mut self, ptr: UntypedPtr, layout: Layout) -> Result<()>;
}

/// represents the heap used by the kernel
pub struct KernelHeap {
    /// start addr of the heap
    start: UntypedPtr,
    /// end addr of the heap (inclusive)
    end: UntypedPtr,
    /// the [SlabAllocator]s used by this heap
    slab_allocators:
        [SlabAllocator<'static, LockedAllocator<LinkedHeap>>; SLAB_ALLOCATOR_SIZES_BYTES.len()],
    /// a linked list allocator for allocations that can't be provided by the [SlabAllocator]s.
    /// also used to allocate the [SlabBlock]s
    linked_heap: LockedAllocator<LinkedHeap>,

    #[cfg(feature = "mem-stats")]
    stats: Option<HeapStats>,
}

unsafe impl Send for KernelHeap {}
unsafe impl Sync for KernelHeap {}

impl KernelHeap {
    /// returns a static ref to the [KernelHeap] lock.
    pub fn get() -> &'static UnwrapTicketLock<KernelHeap> {
        &KERNEL_HEAP
    }

    /// returns stats about the heap.
    ///
    /// This panics if [Self::init_stats] was not called
    #[cfg(feature = "mem-stats")]
    pub fn stats(&self) -> &HeapStats {
        self.stats.as_ref().unwrap()
    }
}

impl KernelHeap {
    /// creates a usable heap for the given pages.
    ///
    /// The pages must not start at vaddr 0.
    ///
    /// # Safety
    ///
    /// Caller ensures that
    /// 1. [KernelHeap::init] is the first function called
    ///     on this struct. In order to do that, the heap must be moved to static memory
    /// 2. `pages` is properly mapped for the kernel to read and write
    unsafe fn new<S: PageSize>(pages: Pages<S>) -> Self {
        trace!("KernelHeap::new()");
        let heap = LockedAllocator {
            allocator: TicketLock::new(LinkedHeap::empty()),
        };

        unsafe {
            heap.allocator
                .lock()
                // safety: called only once and the range passed is valid memory
                // not used for anything else
                .init(pages.start_addr().as_mut_ptr(), pages.size() as usize);
        }

        trace!("create slab allocators...");
        let mut slabs = [const { MaybeUninit::uninit() }; SLAB_ALLOCATOR_SIZES_BYTES.len()];
        for (slab, size) in slabs.iter_mut().zip(SLAB_ALLOCATOR_SIZES_BYTES.iter()) {
            // safety: our safety guarantees, that [KernelHeap::init] is called
            // before anything else, and there we call `nwe_slab.init`.
            let new_slab = unsafe { SlabAllocator::new(*size) };
            slab.write(new_slab);
        }

        unsafe {
            // Safety: we just initialized the arry in the for loop above
            let slab_allocators = MaybeUninit::array_assume_init(slabs);

            // Safety: see function safety
            let start = UntypedPtr::new_from_page(pages.first_page).expect("Expected non 0 page");
            let end = UntypedPtr::new(pages.end_addr()).expect("End vaddr should never be 0");

            trace!("construct heap data structure");
            Self {
                start,
                end,
                slab_allocators,
                linked_heap: heap,
                #[cfg(feature = "mem-stats")]
                stats: None,
            }
        }
    }

    /// init the kernel allocator. This should be called after the allocator
    /// was moved to static memory
    ///
    /// # Safety
    ///
    /// The caller must gurantee that self is never moved after this call.
    unsafe fn init(&mut self) -> Result<()> {
        // Safety: caller gurantees that self is never moved so we can take a static ref.
        //  We don't require &'static mut self, because we don't require self to be borrowed
        //  mutable for a static lifetime. This is ok (not really) because linked_heap
        //  is stored within a TicketLock.
        let linked_heap: &'static _ = unsafe { &*(&self.linked_heap as *const _) };

        for slab in self.slab_allocators.iter_mut() {
            slab.init(linked_heap)?;
        }
        Ok(())
    }

    #[cfg(feature = "mem-stats")]
    fn init_stats<L: LockCell<Self>>(locked_heap: &L, empty_stats: HeapStats) {
        debug!("init heap stats");
        locked_heap.lock().stats = Some(empty_stats);
        trace!("init heap stats allocated");
    }
}

impl MutAllocator for KernelHeap {
    fn alloc(&mut self, layout: Layout) -> Result<UntypedPtr> {
        trace!("alloc {}", layout.size());
        if layout.size() == 0 {
            return Err(MemError::ZeroSizeAllocation);
        }

        #[cfg(feature = "mem-stats")]
        if let Some(stats) = self.stats.as_mut() {
            stats.register_alloc(layout.size());
        }

        for slab in &mut self.slab_allocators {
            if slab.size >= layout.size() {
                return slab.alloc(layout);
            }
        }

        #[cfg(feature = "mem-stats")]
        if let Some(stats) = self.stats.as_mut() {
            stats.register_slab_miss();
        }

        self.linked_heap.alloc(layout)
    }

    unsafe fn free(&mut self, ptr: UntypedPtr, layout: Layout) -> Result<()> {
        if ptr < self.start || ptr + layout.size() > self.end {
            return Err(MemError::PtrNotAllocated(ptr));
        }

        #[cfg(feature = "mem-stats")]
        if let Some(stats) = self.stats.as_mut() {
            stats.register_free(layout.size());
        }

        for slab in &mut self.slab_allocators {
            if slab.size >= layout.size() {
                // safety: same guarantee as ours
                return unsafe { slab.free(ptr, layout) };
            }
        }

        // safety: same guarantee as ours
        unsafe { self.linked_heap.free(ptr, layout) }
    }
}

impl Allocator for UnwrapTicketLock<KernelHeap> {
    fn alloc(&self, layout: Layout) -> Result<UntypedPtr> {
        // TODO looking at the current implementation we don't need this lock
        self.lock().alloc(layout)
    }

    unsafe fn free(&self, ptr: UntypedPtr, layout: Layout) -> Result<()> {
        // TODO looking at the current implementation we don't need this lock
        unsafe { self.lock().free(ptr, layout) }
    }
}

/// A slab allocator is an [MutAllocator] that can allocates fixed size blocks
/// of memory
#[derive(Debug)]
struct SlabAllocator<'a, A> {
    /// Size of the allocations this slab allocator provides
    size: usize,
    /// The first block for tracking allocations (linked list)
    block: Option<NonNull<SlabBlock>>,

    /// the allocator used to alloc blocks
    allocator: MaybeUninit<&'a A>,
}

unsafe impl<A> Send for SlabAllocator<'_, A> {}

impl<'a, A: Allocator> SlabAllocator<'a, A> {
    /// utility to verify size of self is valid
    #[inline]
    fn verify_size(&self) {
        assert!(self.size.is_power_of_two());
    }

    /// utility to verify align of self is valid
    #[inline]
    fn align(&self) -> usize {
        self.size
    }

    /// returns `true` if this allocator can allocate memory with the given alignment
    #[inline]
    fn can_accept_align(&self, align: usize) -> bool {
        assert!(align.is_power_of_two());
        // alignments are always a powe of 2, so if requested align
        // is less or equal to our align it will always match up.
        // Requested 8, our 16 => 16 is always aligned to 8 as well
        align <= self.align()
    }

    /// creates a new Slab Allocator
    ///
    /// # Safety
    ///
    /// caller ensures that [SlabAllocator::init] is the first function called
    /// on self.
    unsafe fn new(size: usize) -> Self {
        let slab = Self {
            size,
            block: None,
            allocator: MaybeUninit::uninit(),
        };
        slab.verify_size();
        slab
    }

    /// initializes the Slab Allocator
    fn init(&mut self, allocator: &'a A) -> Result<()> {
        // TODO allocator should be a pointer and not this
        self.allocator.write(allocator);
        self.block = Some(SlabBlock::new(self.size, allocator)?);

        Ok(())
    }

    /// getter for the internal allocator
    #[inline]
    fn allocator(&self) -> &A {
        // safety: guranteed by the `new`'s safety gurantees
        unsafe { self.allocator.assume_init() }
    }

    /// returns the first block of this [SlabAllocator]
    #[inline]
    fn first_block(&mut self) -> Option<&mut SlabBlock> {
        // safety: we only store valid references in block
        unsafe { self.block.map(|mut b| b.as_mut()) }
    }

    /// returns the first block of this [SlabAllocator] or creates a new
    /// one if none exists
    fn first_block_or_push(&mut self) -> Result<&mut SlabBlock> {
        if let Some(mut block_ptr) = self.block {
            // safety: we only store valid references in block
            unsafe { Ok(block_ptr.as_mut()) }
        } else {
            self.push_new_block()
        }
    }

    /// returns the first block of this [SlabAllocator] or creates one
    /// if no block exists
    #[inline]
    fn push_new_block(&mut self) -> Result<&mut SlabBlock> {
        let mut block_ptr = SlabBlock::new(self.size, self.allocator())?;
        // safety: `SlabBlock:new` returns a valid reference and we have
        // unique access, because we own the ptr and haven't shared it yet
        let block = unsafe { block_ptr.as_mut() };

        block.next = self.block;

        if let Some(mut first_ptr) = self.block {
            // safety: we only store valid references in block
            let first = unsafe { first_ptr.as_mut() };
            first.prev = Some(block_ptr);
        }

        self.block = Some(block_ptr);

        Ok(block)
    }
}

impl<'a, A: Allocator> MutAllocator for SlabAllocator<'a, A> {
    fn alloc(&mut self, layout: Layout) -> Result<UntypedPtr> {
        if layout.size() > self.size {
            return Err(MemError::InvalidAllocSize {
                size: layout.size(),
                expected: self.size,
            });
        }
        if !self.can_accept_align(layout.align()) {
            return Err(MemError::InvalidAllocAlign {
                align: layout.align(),
                expected: self.align(),
            });
        }
        let size = self.size;

        let block = match self.first_block_or_push()?.find_block_with_space(size) {
            Some(block) => block,
            None => self.push_new_block()?,
        };

        block.alloc(size)
    }

    unsafe fn free(&mut self, ptr: UntypedPtr, layout: Layout) -> Result<()> {
        if layout.size() > self.size {
            return Err(MemError::InvalidAllocSize {
                size: layout.size(),
                expected: self.size,
            });
        }
        if !self.can_accept_align(layout.align()) {
            return Err(MemError::InvalidAllocAlign {
                align: layout.align(),
                expected: self.align(),
            });
        }

        let size = self.size;

        let block = self
            .first_block()
            .ok_or(MemError::FreeFailed(ptr))?
            .find_block_containing(ptr)
            .ok_or(MemError::PtrNotAllocated(ptr))?;

        block.freed += size;
        let mut next_first = None;

        if block.is_freed(size) {
            let slab_ptr = unsafe {
                // Safety: block is mapped in current context
                UntypedPtr::new_from_raw(block as *mut SlabBlock as *mut u8).unwrap()
            };

            if let Some(mut prev_ptr) = block.prev {
                // safety: we only store valid references and have mut access,
                // because we have mut access to self
                let prev = unsafe { prev_ptr.as_mut() };
                prev.next = block.next;
            } else {
                // if prev is none, it means it is the first block in the list
                next_first = Some(block.next.clone());
            }
            if let Some(mut next_ptr) = block.next {
                // safety: we only store valid references and have mut access,
                // because we have mut access to self
                let next = unsafe { next_ptr.as_mut() };
                next.prev = block.prev
            }

            if let Some(next_first) = next_first {
                self.block = next_first;
            }

            // only free at the end, so that we ensure a consistent
            // linked list state, even if the free fails
            let free_layout = SlabBlock::layout();
            unsafe {
                // Safety: same guarantee as this function
                self.allocator().free(slab_ptr, free_layout)?;
            }
        }

        Ok(())
    }
}

/// a block of memory used by [SlabAllocator]s to allocate smaller blocks
///
/// This struct represents the header of a block. [`SlabBlock::new`] allocate
/// [SLAB_BLOCK_SIZE] bytes and fits this struct at the start of the allocation.
/// [`SlabBlock::start`] and [`SlabBlock::end`] point to the start and end of the
/// rest of the allocation.
#[derive(Debug)]
struct SlabBlock {
    /// the next block in the linked list
    next: Option<NonNull<SlabBlock>>,
    /// the previous block in the linked list
    prev: Option<NonNull<SlabBlock>>,
    /// the start of memory managed by this block
    start: UntypedPtr,
    /// the end of memory managed by this block
    end: UntypedPtr,
    /// the memory used from this block
    used: usize,
    /// the size of allocations freed from this block
    freed: usize,
}

impl SlabBlock {
    /// helper function to create a [Layout] for a [SlabBlock]
    #[inline]
    fn layout() -> Layout {
        Layout::from_size_align(SLAB_BLOCK_SIZE as usize, align_of::<SlabBlock>())
            .expect("Bug in SlabAllocator block layout calculation")
    }

    /// allocates a new [SlabBlock]. This can fail if the provided `allocator`
    /// fails to allocate enough memory.
    ///
    /// The allocation is of size [SLAB_BLOCK_SIZE] and the [SlabBlock] structure
    /// is at the start of the allocation.
    /// [`SlabBlock::start`] and [`SlabBlock::end`] point to the start and end of the
    /// rest of the allocation.
    fn new<A: Allocator>(size: usize, allocator: &A) -> Result<NonNull<Self>> {
        let layout = Self::layout();
        let block_start = allocator.alloc(layout)?;
        let free_start = (block_start + size_of::<SlabBlock>()).align_up(size);
        let free_end = block_start + (SLAB_BLOCK_SIZE - 1);

        // safety: we just allocated this memory so we can access it however we want
        let block: &mut SlabBlock = unsafe { block_start.as_mut() };

        block.next = None;
        block.prev = None;
        block.start = free_start;
        block.end = free_end;
        block.used = 0;
        block.freed = 0;

        Ok(NonNull::from(block))
    }

    /// returns `true` if the `addr` is within this block.
    #[inline]
    fn contains(&self, addr: UntypedPtr) -> bool {
        addr >= self.start && addr < self.start + SLAB_BLOCK_SIZE
    }

    /// tries to find a the [SlabBlock] that contains the `addr`. If this block
    /// doesn't contain the `addr` it checks [SlabBlock::next].
    fn find_block_containing(&mut self, ptr: UntypedPtr) -> Option<&mut Self> {
        if self.contains(ptr) {
            Some(self)
        } else if let Some(mut next) = self.next {
            // safety: we onyl store valid references and we have mut access,
            // because we have mut access to self
            unsafe { next.as_mut().find_block_containing(ptr) }
        } else {
            None
        }
    }

    /// returns `true` if the block has no more space for a `size` allocation
    #[inline]
    fn is_full(&self, size: usize) -> bool {
        self.start + self.used + size >= self.end
    }

    /// returns `true` if all possible allocations of `size` have been freed
    #[inline]
    fn is_freed(&self, size: usize) -> bool {
        self.start + self.freed + size >= self.end
    }

    /// finds the first block that can still fit `size`
    fn find_block_with_space(&mut self, size: usize) -> Option<&mut SlabBlock> {
        if !self.is_full(size) {
            Some(self)
        } else if let Some(mut next) = self.next {
            // safety: we onyl store valid references and we have mut access,
            // because we have mut access to self
            unsafe { next.as_mut().find_block_with_space(size) }
        } else {
            None
        }
    }

    /// allocates `size` bytes from this block
    fn alloc(&mut self, size: usize) -> Result<UntypedPtr> {
        if self.is_full(size) {
            return Err(MemError::OutOfMemory);
        }
        if size == 0 {
            return Err(MemError::ZeroSizeAllocation);
        }

        let start = self.start + self.used;
        assert!(start + size <= self.end);
        self.used += size;

        assert!(start.is_aligned(size));

        // safety:
        // start + used ..=end is unused memory.
        // We take the next size bytes from this region and return this as a new
        // block of memory. We increment start in order to ensure that the
        // prerequisite holds in the future
        Ok(start)
    }
}

impl<A: Allocator> MutAllocator for A {
    fn alloc(&mut self, layout: Layout) -> Result<UntypedPtr> {
        Allocator::alloc(self, layout)
    }

    unsafe fn free(&mut self, ptr: UntypedPtr, layout: Layout) -> Result<()> {
        // Safety: same as [MutAllocator::free]
        unsafe { Allocator::free(self, ptr, layout) }
    }
}

impl MutAllocator for LinkedHeap {
    fn alloc(&mut self, layout: Layout) -> Result<UntypedPtr> {
        self.allocate_first_fit(layout)
            .map(|ptr| ptr.into())
            .map_err(|_| MemError::OutOfMemory)
    }

    unsafe fn free(&mut self, ptr: UntypedPtr, layout: Layout) -> Result<()> {
        // safety: same gaurantee as this function
        unsafe { self.deallocate(ptr.into_inner(), layout) };
        Ok(())
    }
}

/// a wrapper around a [TicketLock] containing an [MutAllocator]
struct LockedAllocator<A> {
    allocator: TicketLock<A>,
}

impl<A: MutAllocator + Send> Allocator for LockedAllocator<A> {
    fn alloc(&self, layout: Layout) -> Result<UntypedPtr> {
        self.allocator.lock().alloc(layout)
    }

    unsafe fn free(&self, ptr: UntypedPtr, layout: Layout) -> Result<()> {
        unsafe { self.allocator.lock().free(ptr, layout) }
    }
}
