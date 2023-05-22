use core::alloc::Layout;
use core::ops::DerefMut;
use core::ptr::Alignment;

use shared::lockcell::{LockCell, SpinLock};

use super::{MemError, PhysAddr};

pub struct PhysBlock {
    addr: PhysAddr,
    layout: Layout,
}

trait PhysAllocator {
    fn min_align() -> Alignment {
        Alignment::MIN
    }

    fn alloc(&mut self, layout: Layout) -> Result<PhysBlock, MemError>;

    fn alloc_zerod(&mut self, layout: Layout) -> Result<PhysBlock, MemError>;

    fn alloc_init_magic(
        &mut self,
        layout: Layout,
        magic_value: &[u8],
    ) -> Result<PhysBlock, MemError>;

    fn dealloc(&mut self, block: PhysBlock);

    fn grow(&mut self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError>;

    fn grow_zeroed(&mut self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError>;

    fn grow_init_magic(
        &mut self,
        block: PhysBlock,
        new_layout: Layout,
        magic_value: &[u8],
    ) -> Result<PhysBlock, MemError>;

    fn shrink(&mut self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError>;
}

trait PhysSyncAllocator {
    fn min_align() -> Alignment {
        Alignment::MIN
    }

    fn alloc(&self, layout: Layout) -> Result<PhysBlock, MemError>;

    fn alloc_zerod(&self, layout: Layout) -> Result<PhysBlock, MemError>;

    fn alloc_init_magic(&self, layout: Layout, magic_value: &[u8]) -> Result<PhysBlock, MemError>;

    fn dealloc(&self, block: PhysBlock);

    fn grow(&self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError>;

    fn grow_zeroed(&self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError>;

    fn grow_init_magic(
        &self,
        block: PhysBlock,
        new_layout: Layout,
        magic_value: &[u8],
    ) -> Result<PhysBlock, MemError>;

    fn shrink(&self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError>;
}

impl<A> PhysAllocator for A
where
    A: PhysSyncAllocator,
{
    fn alloc(&mut self, layout: Layout) -> Result<PhysBlock, MemError> {
        PhysSyncAllocator::alloc(self, layout)
    }

    fn alloc_zerod(&mut self, layout: Layout) -> Result<PhysBlock, MemError> {
        PhysSyncAllocator::alloc_zerod(self, layout)
    }

    fn alloc_init_magic(
        &mut self,
        layout: Layout,
        magic_value: &[u8],
    ) -> Result<PhysBlock, MemError> {
        PhysSyncAllocator::alloc_init_magic(self, layout, magic_value)
    }

    fn dealloc(&mut self, block: PhysBlock) {
        PhysSyncAllocator::dealloc(self, block)
    }

    fn grow(&mut self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError> {
        PhysSyncAllocator::grow(self, block, new_layout)
    }

    fn grow_zeroed(&mut self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError> {
        PhysSyncAllocator::grow_zeroed(self, block, new_layout)
    }

    fn grow_init_magic(
        &mut self,
        block: PhysBlock,
        new_layout: Layout,
        magic_value: &[u8],
    ) -> Result<PhysBlock, MemError> {
        PhysSyncAllocator::grow_init_magic(self, block, new_layout, magic_value)
    }

    fn shrink(&mut self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError> {
        PhysSyncAllocator::shrink(self, block, new_layout)
    }
}

macro_rules! sync_alloc_from_no_sync {
    ($mutex:ident) => {
        impl<A: PhysAllocator> PhysSyncAllocator for $mutex<A> {
            fn alloc(&self, layout: Layout) -> Result<PhysBlock, MemError> {
                PhysAllocator::alloc(self.lock().deref_mut(), layout)
            }

            fn alloc_zerod(&self, layout: Layout) -> Result<PhysBlock, MemError> {
                PhysAllocator::alloc_zerod(self.lock().deref_mut(), layout)
            }

            fn alloc_init_magic(
                &self,
                layout: Layout,
                magic_value: &[u8],
            ) -> Result<PhysBlock, MemError> {
                PhysAllocator::alloc_init_magic(self.lock().deref_mut(), layout, magic_value)
            }

            fn dealloc(&self, block: PhysBlock) {
                PhysAllocator::dealloc(self.lock().deref_mut(), block)
            }

            fn grow(&self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError> {
                PhysAllocator::grow(self.lock().deref_mut(), block, new_layout)
            }

            fn grow_zeroed(
                &self,
                block: PhysBlock,
                new_layout: Layout,
            ) -> Result<PhysBlock, MemError> {
                PhysAllocator::grow_zeroed(self.lock().deref_mut(), block, new_layout)
            }

            fn grow_init_magic(
                &self,
                block: PhysBlock,
                new_layout: Layout,
                magic_value: &[u8],
            ) -> Result<PhysBlock, MemError> {
                PhysAllocator::grow_init_magic(
                    self.lock().deref_mut(),
                    block,
                    new_layout,
                    magic_value,
                )
            }

            fn shrink(&self, block: PhysBlock, new_layout: Layout) -> Result<PhysBlock, MemError> {
                PhysAllocator::shrink(self.lock().deref_mut(), block, new_layout)
            }
        }
    };
}

sync_alloc_from_no_sync!(SpinLock);
