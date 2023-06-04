#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::{
    mem::{
        frame_allocator::WasabiFrameAllocator, page_allocator::PageAllocator,
        page_table::KernelPageTable,
    },
    prelude::{LockCell, SpinLock},
};

use super::{page_allocator::Pages, MemError, Result};
use core::{
    alloc::{GlobalAlloc, Layout},
    mem::{align_of, size_of},
    ops::DerefMut,
    ptr::{null_mut, NonNull},
};
use lazy_static::lazy_static;
use linked_list_allocator::Heap as LinkedHeap;
use shared::sizes::KiB;
use x86_64::{
    structures::paging::{Mapper, PageSize, PageTableFlags, Size4KiB},
    VirtAddr,
};

lazy_static! {
    // safety: we are initializing static mem so KernelHeap::empty is ok
    static ref KERNEL_HEAP: SpinLock<KernelHeap> = unsafe { SpinLock::new(KernelHeap::empty()) };
}

struct KernelHeapGlobalAllocator;
#[global_allocator]
static GLOBAL_ALLOCATOR: KernelHeapGlobalAllocator = KernelHeapGlobalAllocator;

unsafe impl GlobalAlloc for KernelHeapGlobalAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        trace!(target: "GlobalAlloc", "allocate {layout:?}");
        match KernelHeap::get().alloc(layout) {
            Ok(mut mem) => mem.as_mut(),
            Err(err) => {
                error!("Kernel Heap allocation failed for layout {layout:?}: {err:?}");
                null_mut()
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        trace!(target: "GlobalAlloc", "free {ptr:p} - {layout:?}");
        if let Some(non_null) = NonNull::new(ptr) {
            match KernelHeap::get().free(non_null, layout) {
                Ok(_) => {}
                Err(err) => error!("Failed to free {non_null:p} with layout {layout:?}: {err:?}"),
            }
        } else {
            error!("tried to free null pointer with layout {layout:?}");
        }
    }
}

pub fn init() {
    info!("init kernel heap");

    let pages = PageAllocator::get_kernel_allocator()
        .lock()
        .allocate_pages::<Size4KiB>(KERNEL_HEAP_PAGE_COUNT)
        .expect("Out of pages setting up kernel heap");

    let mut page_table = KernelPageTable::get().lock();

    let mut frame_allocator = WasabiFrameAllocator::<Size4KiB>::get_for_kernel().lock();
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

    for page in pages.iter() {
        let frame = frame_allocator
            .alloc()
            .expect("Out of memory setting up kernel heap");
        unsafe {
            match page_table.map_to(page, frame, flags, frame_allocator.deref_mut()) {
                Ok(flusher) => flusher.flush(),
                Err(e) => panic!("Failed to map page {page:?} to frame {frame:?}: {e:?}"),
            }
            trace!("Page {page:?} mapped to {frame:?}");
        }
    }

    let heap = KernelHeap::get();
    let mut guard = heap.lock();
    if let Err(e) = guard.init(pages) {
        panic!("KernelHeap::init(): {e:?}");
    }

    trace!("kernel init done");
}

const KERNEL_HEAP_PAGE_COUNT: usize = 5;

const SLAB_ALLOCATOR_SIZES_BYTES: [usize; 5] = [2, 4, 8, 16, 32];
const SLAB_BLOCK_SIZE: usize = KiB(1);

trait Allocator {
    fn alloc(&self, layout: Layout) -> Result<NonNull<u8>>;
    unsafe fn free(&self, ptr: NonNull<u8>, layout: Layout) -> Result<()>;
}

trait MutAllocator {
    fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>>;
    unsafe fn free(&mut self, ptr: NonNull<u8>, layout: Layout) -> Result<()>;
}

impl<A: Allocator> MutAllocator for A {
    fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>> {
        Allocator::alloc(self, layout)
    }

    unsafe fn free(&mut self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
        Allocator::free(self, ptr, layout)
    }
}

pub struct KernelHeap {
    start: VirtAddr,
    end: VirtAddr,
    slab_allocators:
        [SlabAllocator<'static, LockedAllocator<LinkedHeap>>; SLAB_ALLOCATOR_SIZES_BYTES.len()],
    linked_heap: LockedAllocator<LinkedHeap>,
}

impl KernelHeap {
    pub fn get() -> &'static SpinLock<KernelHeap> {
        &KERNEL_HEAP
    }
}

impl KernelHeap {
    /// Safety: This must only be used to initialize static memory
    unsafe fn empty() -> Self {
        let heap = LockedAllocator {
            allocator: SpinLock::new(LinkedHeap::empty()),
        };
        let slabs = [
            SlabAllocator::empty(),
            SlabAllocator::empty(),
            SlabAllocator::empty(),
            SlabAllocator::empty(),
            SlabAllocator::empty(),
        ];
        Self {
            start: VirtAddr::zero(),
            end: VirtAddr::zero(),
            slab_allocators: slabs,
            linked_heap: heap,
        }
    }

    fn init<S: PageSize>(&mut self, pages: Pages<S>) -> Result<()> {
        trace!("KernelHeap::init()");
        self.start = pages.start_addr();
        self.end = pages.end_addr();

        unsafe {
            self.linked_heap
                .allocator
                .lock()
                .init(pages.start_addr().as_mut_ptr(), pages.size() as usize);
        }

        let static_heap = unsafe { &*(&self.linked_heap as *const LockedAllocator<_>) };

        for i in 0..SLAB_ALLOCATOR_SIZES_BYTES.len() {
            let size = SLAB_ALLOCATOR_SIZES_BYTES[i];
            let slab = &mut self.slab_allocators[i];

            trace!("init slab for size {size}");
            slab.init(size, static_heap)?;
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
                return slab.free(ptr, layout);
            }
        }

        self.linked_heap.free(ptr, layout)
    }
}

impl Allocator for SpinLock<KernelHeap> {
    fn alloc(&self, layout: Layout) -> Result<NonNull<u8>> {
        self.lock().alloc(layout)
    }

    unsafe fn free(&self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
        self.lock().free(ptr, layout)
    }
}

#[derive(Debug)]
struct SlabAllocator<'a, A> {
    /// Size of the allocations this slab allocator provides
    size: usize,
    /// The first block for tracking allocations (linked list)
    block: Option<NonNull<SlabBlock>>,

    /// the allocator used to alloc blocks
    allocator: Option<&'a A>,
}

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
    fn layout() -> Layout {
        Layout::from_size_align(SLAB_BLOCK_SIZE, align_of::<SlabBlock>())
            .expect("Bug in SlabAllocator block layout calculation")
    }

    fn new<A: Allocator>(size: usize, allocator: &A) -> Result<NonNull<Self>> {
        let layout = Self::layout();
        let memory = allocator.alloc(layout)?;
        let block_start = VirtAddr::from_ptr(memory.as_ptr());
        let free_start = (block_start + size_of::<SlabBlock>()).align_up(size as u64);
        let free_end = block_start + (SLAB_BLOCK_SIZE - 1);

        let block = unsafe { &mut *(memory.as_ptr() as *mut SlabBlock) };

        block.next = None;
        block.prev = None;
        block.start = free_start;
        block.end = free_end;
        block.used = 0;
        block.freed = 0;

        Ok(NonNull::from(block))
    }

    fn contains(&self, addr: VirtAddr) -> bool {
        addr >= self.start && addr < self.start + SLAB_BLOCK_SIZE
    }

    fn find_block_containing(&mut self, addr: VirtAddr) -> Option<&mut Self> {
        if self.contains(addr) {
            Some(self)
        } else if let Some(mut next) = self.next {
            unsafe { next.as_mut().find_block_containing(addr) }
        } else {
            None
        }
    }

    fn is_full(&self, size: usize) -> bool {
        self.start + self.used + size >= self.end
    }

    fn is_freed(&self, size: usize) -> bool {
        self.start + self.freed + size >= self.end
    }

    fn find_block_with_space(&mut self, size: usize) -> Option<&mut SlabBlock> {
        if !self.is_full(size) {
            Some(self)
        } else if let Some(mut next) = self.next {
            unsafe { next.as_mut().find_block_with_space(size) }
        } else {
            None
        }
    }

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

        unsafe { Ok(NonNull::new_unchecked(start.as_mut_ptr())) }
    }
}

impl<'a, A: Allocator> SlabAllocator<'a, A> {
    #[inline]
    fn verify_size(&self) {
        assert!(self.size.is_power_of_two());
    }

    #[inline]
    fn align(&self) -> usize {
        self.size
    }

    #[inline]
    fn can_accept_align(&self, align: usize) -> bool {
        assert!(align.is_power_of_two());
        // alignments are always a powe of 2, so if requested align
        // is less or equal to our align it will always match up.
        // Requested 8, our 16 => 16 is always aligned to 8 as well
        align <= self.align()
    }

    fn empty() -> Self {
        SlabAllocator {
            size: 0,
            block: None,
            allocator: None,
        }
    }

    fn init(&mut self, size: usize, allocator: &'a A) -> Result<()> {
        self.size = size;
        self.allocator = Some(allocator);
        self.block = Some(SlabBlock::new(size, allocator)?);
        self.verify_size();
        Ok(())
    }

    fn first_block(&mut self) -> Option<&mut SlabBlock> {
        unsafe { self.block.map(|mut b| b.as_mut()) }
    }

    fn first_block_or_push(&mut self) -> Result<&mut SlabBlock> {
        if let Some(mut block_ptr) = self.block {
            unsafe { Ok(block_ptr.as_mut()) }
        } else {
            self.push_new_block()
        }
    }

    fn push_new_block(&mut self) -> Result<&mut SlabBlock> {
        let allocator = self.allocator.ok_or(MemError::NotInit)?;

        let mut block_ptr = SlabBlock::new(self.size, allocator)?;
        let block = unsafe { block_ptr.as_mut() };

        block.next = self.block;

        if let Some(mut first_ptr) = self.block {
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
        let allocator = self.allocator.ok_or(MemError::NotInit)?;

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
                let prev = unsafe { prev_ptr.as_mut() };
                prev.next = block.next;
            } else {
                // if prev is none, it means it is the first block in the list
                next_first = Some(block.next.clone());
            }
            if let Some(mut next_ptr) = block.next {
                let next = unsafe { next_ptr.as_mut() };
                next.prev = block.prev
            }

            if let Some(next_first) = next_first {
                self.block = next_first;
            }

            // only free at the end, so that we ensure a consistent
            // linked list state, even if the free fails
            let free_layout = SlabBlock::layout();
            allocator.free(slab_ptr, free_layout)?;
        }

        Ok(())
    }
}

impl MutAllocator for LinkedHeap {
    fn alloc(&mut self, layout: Layout) -> Result<NonNull<u8>> {
        self.allocate_first_fit(layout)
            .map_err(|_| MemError::OutOfMemory)
    }

    unsafe fn free(&mut self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
        self.deallocate(ptr, layout);
        Ok(())
    }
}

struct LockedAllocator<A> {
    allocator: SpinLock<A>,
}

impl<A: MutAllocator> Allocator for LockedAllocator<A> {
    fn alloc(&self, layout: Layout) -> Result<NonNull<u8>> {
        self.allocator.lock().alloc(layout)
    }

    unsafe fn free(&self, ptr: NonNull<u8>, layout: Layout) -> Result<()> {
        self.allocator.lock().free(ptr, layout)
    }
}
