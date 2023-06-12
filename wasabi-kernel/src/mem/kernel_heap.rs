//! provides [GlobalAlloc] for the kernel

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::{
    mem::{
        frame_allocator::WasabiFrameAllocator,
        page_allocator::{PageAllocator, Pages},
        page_table::KERNEL_PAGE_TABLE,
        MemError, Result,
    },
    prelude::{LockCell, TicketLock, UnwrapTicketLock},
};

use core::{
    alloc::{GlobalAlloc, Layout},
    mem::{align_of, size_of, MaybeUninit},
    ops::DerefMut,
    ptr::{null_mut, NonNull},
};
use linked_list_allocator::Heap as LinkedHeap;
use shared::{lockcell::LockCellInternal, sizes::KiB};
use x86_64::{
    structures::paging::{Mapper, PageSize, PageTableFlags, Size4KiB},
    VirtAddr,
};

/// the [KernelHeap]
// Safety: initialized by [init] before it we use allocated types
static KERNEL_HEAP: UnwrapTicketLock<KernelHeap> = unsafe { UnwrapTicketLock::new_uninit() };

/// holds ZST for [global_allocator]
#[global_allocator]
static GLOBAL_ALLOCATOR: KernelHeapGlobalAllocator = KernelHeapGlobalAllocator;

/// initializes the kernel heap
pub fn init() {
    info!("init kernel heap");

    let pages = PageAllocator::get_kernel_allocator()
        .lock()
        .allocate_pages::<Size4KiB>(KERNEL_HEAP_PAGE_COUNT)
        .expect("Out of pages setting up kernel heap");

    {
        let mut page_table = KERNEL_PAGE_TABLE.lock();

        let mut frame_allocator = WasabiFrameAllocator::<Size4KiB>::get_for_kernel().lock();
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

        for page in pages.iter() {
            let frame = frame_allocator
                .alloc()
                .expect("Out of memory setting up kernel heap");
            unsafe {
                // Safety: we are mapping new unused pages to new unused phys frames
                match page_table.map_to(page, frame, flags, frame_allocator.deref_mut()) {
                    Ok(flusher) => flusher.flush(),
                    Err(e) => panic!("Failed to map page {page:?} to frame {frame:?}: {e:?}"),
                }
                trace!("Page {page:?} mapped to {frame:?}");
            }
        }
    }

    let mut kernel_lock = KERNEL_HEAP.lock_uninit();
    unsafe {
        // Safety: we call init right after we move this to static storage
        kernel_lock.write(KernelHeap::new(pages));
    }

    // Safety: we hold the unit_lock so accessing the heap is valid.
    // we don't use the lock so we can get at the static lifetime
    let heap: &mut KernelHeap = unsafe { UnwrapTicketLock::get_mut(&KERNEL_HEAP) };
    if let Err(e) = heap.init() {
        panic!("KernelHeap::new(): {e:?}")
    };

    trace!("kernel init done");
}

/// ZST for [GlobalAlloc] implementation
struct KernelHeapGlobalAllocator;

unsafe impl GlobalAlloc for KernelHeapGlobalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        trace!(target: "GlobalAlloc", "allocate {layout:?}");
        match KernelHeap::get().alloc(layout) {
            Ok(mut mem) => unsafe {
                // Safety: [KernelHeap::alloc] returns a valid pointer
                // and we still have unique access to it
                mem.as_mut()
            },
            Err(err) => {
                error!("Kernel Heap allocation failed for layout {layout:?}: {err:?}");
                null_mut()
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        trace!(target: "GlobalAlloc", "free {ptr:p} - {layout:?}");
        if let Some(non_null) = NonNull::new(ptr) {
            // Safety: see safety guarantees of [GlobalAlloc]
            match unsafe { KernelHeap::get().free(non_null, layout) } {
                Ok(_) => {}
                Err(err) => error!("Failed to free {non_null:p} with layout {layout:?}: {err:?}"),
            }
        } else {
            error!("tried to free null pointer with layout {layout:?}");
        }
    }
}

/// number of memory pages used by the kernel heap
const KERNEL_HEAP_PAGE_COUNT: usize = 5;

/// the sizes of the [SlabAllocator]s used by the kernel heap
const SLAB_ALLOCATOR_SIZES_BYTES: [usize; 5] = [2, 4, 8, 16, 32];

/// block size for the [SlabAllocator]
const SLAB_BLOCK_SIZE: usize = KiB(1);

/// The allocator trait used by the kernel
trait Allocator {
    /// allocate [Layout] and return either a non null [u8] pointer or [MemError]
    fn alloc(&self, layout: Layout) -> Result<NonNull<u8>>;

    /// free a pointer with a given [Layout]. This can fail with [MemError],
    /// e.g. when the `ptr` was not allocated by this [Allocator]
    ///
    /// # Safety:
    ///
    /// the caller must ensure that the ptr and layout match
    /// and that the ptr was allocated by this [Allocator]
    unsafe fn free(&self, ptr: NonNull<u8>, layout: Layout) -> Result<()>;
}

/// Same allocator trait as [Allocator] but requires mut access to `self`
trait MutAllocator {
    /// allocate [Layout] and return either a non null [u8] pointer or [MemError]
    fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>>;

    /// free a pointer with a given [Layout]. This can fail with [MemError],
    /// e.g. when the `ptr` was not allocated by this [Allocator]
    ///
    /// # Safety:
    ///
    /// the caller must ensure that the ptr and layout match
    /// and that the ptr was allocated by this [MutAllocator]
    unsafe fn free(&mut self, ptr: NonNull<u8>, layout: Layout) -> Result<()>;
}

/// represents the heap used by the kernel
pub struct KernelHeap {
    /// start addr of the heap
    start: VirtAddr,
    /// end addr of the heap (inclusive)
    end: VirtAddr,
    /// the [SlabAllocator]s used by this heap
    slab_allocators:
        [SlabAllocator<'static, LockedAllocator<LinkedHeap>>; SLAB_ALLOCATOR_SIZES_BYTES.len()],
    /// a linked list allocator for allocations that can't be provided by the [SlabAllocator]s.
    /// also used to allocate the [SlabBlock]s
    linked_heap: LockedAllocator<LinkedHeap>,
}

impl KernelHeap {
    /// returns a static ref to the [KernelHeap] lock.
    pub fn get() -> &'static UnwrapTicketLock<KernelHeap> {
        &KERNEL_HEAP
    }
}

impl KernelHeap {
    /// creates a usable heap for the given pages
    ///
    /// # Safety
    ///
    /// caller ensures that [KernelHeap::init] is the first function called
    /// on this struct. In order to do that, the heap must be moved to static memory
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

        let mut slabs: [MaybeUninit<_>; SLAB_ALLOCATOR_SIZES_BYTES.len()] =
            MaybeUninit::uninit_array();
        for (slab, size) in slabs.iter_mut().zip(SLAB_ALLOCATOR_SIZES_BYTES.iter()) {
            // safety: our safety guarantees, that [KernelHeap::init] is called
            // before anything else, and there we call `nwe_slab.init`.
            let new_slab = unsafe { SlabAllocator::new(*size) };
            slab.write(new_slab);
        }

        Self {
            start: pages.start_addr(),
            end: pages.end_addr(),
            slab_allocators: unsafe { MaybeUninit::array_assume_init(slabs) },
            linked_heap: heap,
        }
    }

    /// init the kernel allocator. This should be called after the allocator
    /// was moved to static memory
    fn init(&'static mut self) -> Result<()> {
        for slab in self.slab_allocators.iter_mut() {
            slab.init(&self.linked_heap)?;
        }
        Ok(())
    }
}

impl MutAllocator for KernelHeap {
    fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>> {
        if layout.size() == 0 {
            return Err(MemError::ZeroSizeAllocation);
        }
        for slab in &mut self.slab_allocators {
            if slab.size >= layout.size() {
                return slab.alloc(layout);
            }
        }

        self.linked_heap.alloc(layout)
    }

    unsafe fn free(&mut self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
        let vaddr = VirtAddr::from_ptr(ptr.as_ptr());
        if vaddr < self.start || vaddr + layout.size() > self.end {
            return Err(MemError::PtrNotAllocated(ptr));
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
    fn alloc(&self, layout: Layout) -> Result<NonNull<u8>> {
        self.lock().alloc(layout)
    }

    unsafe fn free(&self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
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
    /// # Safety:
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
    fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>> {
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

    unsafe fn free(&mut self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
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

        let addr = VirtAddr::from_ptr(ptr.as_ptr());
        let size = self.size;

        let block = self
            .first_block()
            .ok_or(MemError::FreeFailed(ptr))?
            .find_block_containing(addr)
            .ok_or(MemError::PtrNotAllocated(ptr))?;

        block.freed += size;
        let mut next_first = None;

        if block.is_freed(size) {
            let slab_ptr = NonNull::new(block as *mut SlabBlock as *mut u8).unwrap();

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
    start: VirtAddr,
    /// the end of memory managed by this block
    end: VirtAddr,
    /// the memory used from this block
    used: usize,
    /// the size of allocations freed from this block
    freed: usize,
}

impl SlabBlock {
    /// helper function to create a [Layout] for a [SlabBlock]
    #[inline]
    fn layout() -> Layout {
        Layout::from_size_align(SLAB_BLOCK_SIZE, align_of::<SlabBlock>())
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
        let memory = allocator.alloc(layout)?;
        let block_start = VirtAddr::from_ptr(memory.as_ptr());
        let free_start = (block_start + size_of::<SlabBlock>()).align_up(size as u64);
        let free_end = block_start + (SLAB_BLOCK_SIZE - 1);

        // safety: we just allocated this memory so we can access it however we want
        let block = unsafe { &mut *(memory.as_ptr() as *mut SlabBlock) };

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
    fn contains(&self, addr: VirtAddr) -> bool {
        addr >= self.start && addr < self.start + SLAB_BLOCK_SIZE
    }

    /// tries to find a the [SlabBlock] that contains the `addr`. If this block
    /// doesn't contain the `addr` it checks [SlabBlock::next].
    fn find_block_containing(&mut self, addr: VirtAddr) -> Option<&mut Self> {
        if self.contains(addr) {
            Some(self)
        } else if let Some(mut next) = self.next {
            // safety: we onyl store valid references and we have mut access,
            // because we have mut access to self
            unsafe { next.as_mut().find_block_containing(addr) }
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
    fn alloc(&mut self, size: usize) -> Result<NonNull<u8>> {
        if self.is_full(size) {
            return Err(MemError::OutOfMemory);
        }
        if size == 0 {
            return Err(MemError::ZeroSizeAllocation);
        }

        let start = self.start + self.used;
        assert!(start + size <= self.end);
        self.used += size;

        assert!(start.is_aligned(size as u64));

        // safety:
        // start + used ..=end is unused memory.
        // We take the next size bytes from this region and return this as a new
        // block of memory. We increment start in order to ensure that the
        // prerequisite holds in the future
        unsafe { Ok(NonNull::new_unchecked(start.as_mut_ptr())) }
    }
}

impl<A: Allocator> MutAllocator for A {
    fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>> {
        Allocator::alloc(self, layout)
    }

    unsafe fn free(&mut self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
        // Safety: same as [MutAllocator::free]
        unsafe { Allocator::free(self, ptr, layout) }
    }
}

impl MutAllocator for LinkedHeap {
    fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>> {
        self.allocate_first_fit(layout)
            .map_err(|_| MemError::OutOfMemory)
    }

    unsafe fn free(&mut self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
        // safety: same gaurantee as this function
        unsafe { self.deallocate(ptr, layout) };
        Ok(())
    }
}

/// a wrapper around a [TicketLock] containing an [MutAllocator]
struct LockedAllocator<A> {
    allocator: TicketLock<A>,
}

impl<A: MutAllocator + Send> Allocator for LockedAllocator<A> {
    fn alloc(&self, layout: Layout) -> Result<NonNull<u8>> {
        self.allocator.lock().alloc(layout)
    }

    unsafe fn free(&self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
        unsafe { self.allocator.lock().free(ptr, layout) }
    }
}
