use core::{cmp::max, num::NonZeroU64};

use alloc::{boxed::Box, vec::Vec};
use log::{debug, error, info};
use shared::{counts_required_for, todo_warn};
use staticvec::StaticVec;
use thiserror::Error;

use crate::{
    Block, BlockGroup, LBA, blocks_required_for,
    fs::FsError,
    fs_structs::{BLOCK_RANGES_COUNT_PER_BLOCK, DevicePointer, FreeBlockGroups},
    interface::BlockDevice,
};

#[derive(Clone, Debug)]
pub struct BlockAllocator {
    /// Ranges of blocks that are currently unused
    free: Vec<BlockGroup>,
    /// The first block of the on disk structure that represents this
    /// allocators data
    on_disk: Option<DevicePointer<FreeBlockGroups>>,
    /// all blocks invovled in storing this allocator data
    self_on_disk: Vec<LBA>,
    /// `true` if this is different from the on device version
    dirty: bool,
}

/// The size needed to store the [BlockAllocator] data on disc, depends
/// on the the size of [BlockAllocator::free]. However allocating the
/// blocks required to store the [BlockAllocator] on the disc has a small
/// chance of changing `free` in a way that does not fit in the initial allocation.
///
/// The block allocator will try multiple times to find an allocation that will
/// fit `free` as well as the overhead of the blocks for itself, each time refining
/// the size.
const CALC_ALLOC_SIZE_TRY_COUNT: usize = 10;

// FIXME: there seems to be a bug. Somehow there is not enough space to create an empty dir
// in an empty fs that is 1GB in size. There should be enough space
impl BlockAllocator {
    /// Creates a new [BlockAllocator] that is free, except for `initial_usage`.
    pub fn empty(inital_usage: &[LBA], max_lba: LBA) -> Self {
        let mut this = Self {
            free: Vec::new(),
            on_disk: None,
            self_on_disk: Vec::new(),
            dirty: true,
        };

        this.free.push(BlockGroup {
            start: LBA::new(0).unwrap(),
            count_minus_one: max_lba.get() - 1,
        });

        for block in inital_usage {
            this.mark_used(*block)
                .expect("initial usage should not have any duplicates and be within range");
        }

        this
    }

    /// `true` if there are changes that have not been saved to the device
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn write<D: BlockDevice>(
        &mut self,
        device: &mut D,
    ) -> Result<DevicePointer<FreeBlockGroups>, FsError> {
        debug_assert!(self.check_consistent().is_ok());

        let mut new = self.clone();
        new.self_on_disk.clear();
        new.on_disk = None;

        for block in &self.self_on_disk {
            new.free_block(*block);
        }

        // we need at least 1 block, because the main header does have a concept of
        // "no free list". Instead it just stores a pointer to a block with an empty list.
        let mut free_group_count = max(new.free.len(), 1);
        let blocks_required =
            counts_required_for!(BLOCK_RANGES_COUNT_PER_BLOCK, free_group_count) as u64;

        let mut on_disk_blocks: BlockGroupList;

        let mut loop_count = 0;
        loop {
            on_disk_blocks = new.allocate(blocks_required)?;

            // check if the newley allocated blocks mean that the free list changed so much
            // that our new allocation no longer fits.
            if on_disk_blocks.block_count()
                >= counts_required_for!(BLOCK_RANGES_COUNT_PER_BLOCK, new.free.len()) as u64
            {
                break;
            }
            assert!(
                free_group_count < new.free.len(),
                "the count should be larger, otherwise we should already have enough blocks allcoated"
            );

            // on the next try, try to allocate enough space for the current attempt.
            // This should converge to a successfull attempt
            free_group_count = new.free.len();
            new.free(on_disk_blocks);

            if loop_count >= CALC_ALLOC_SIZE_TRY_COUNT {
                // only try 10 times. If this fails something is wrong with my algorithm

                error!(
                    "Failed to allocate enough blocks to save the BlockAllocator. Stopped after {CALC_ALLOC_SIZE_TRY_COUNT} attempts"
                );
                return Err(FsError::WriteAllocatorFreeList);
            }
            info!("Failed to allocate enough blocks to save the BlockAllocator. Trying again");
            loop_count += 1;
        }

        let mut on_disk_block_iter = on_disk_blocks.block_iter().peekable();
        let first_block = *on_disk_block_iter
            .peek()
            .expect("There should always be at least 1 block");
        for block_data in new.free.chunks(BLOCK_RANGES_COUNT_PER_BLOCK) {
            let lba = on_disk_block_iter.next().unwrap();
            let mut block = Block::new(FreeBlockGroups {
                free: StaticVec::new(),
                next: on_disk_block_iter.peek().map(|n| DevicePointer::new(*n)),
            });

            block.free.extend_from_slice(block_data);

            device
                .write_block(lba, block.block_data())
                .map_err(|e| FsError::BlockDevice(Box::new(e)))?;
        }

        loop {
            // there is a small chance that we allocatd more blocks, than we actually need.
            // We "have" to use them, otherwise we don't know that they can be freed in the future
            let Some(empty_block_lba) = on_disk_block_iter.next() else {
                break;
            };
            let block = Block::new(FreeBlockGroups {
                free: StaticVec::new(),
                next: on_disk_block_iter.peek().map(|n| DevicePointer::new(*n)),
            });

            device
                .write_block(empty_block_lba, block.block_data())
                .map_err(|e| FsError::BlockDevice(Box::new(e)))?;
        }

        new.dirty = false;
        *self = new;
        Ok(DevicePointer::new(first_block))
    }

    pub fn load<D: BlockDevice>(
        device: &D,
        free_blocks: DevicePointer<FreeBlockGroups>,
    ) -> Result<Self, D::BlockDeviceError> {
        let on_disk = Some(free_blocks);
        let mut free = Vec::new();
        let mut self_on_disk = Vec::new();

        let mut next_block = on_disk;
        while let Some(block) = next_block {
            self_on_disk.push(block.lba);

            let data: FreeBlockGroups = device.read_pointer(block)?;

            for group in data.free {
                free.push(group);
            }

            next_block = data.next;
        }

        let block_allocator = Self {
            free,
            on_disk,
            self_on_disk,
            dirty: false,
        };

        debug_assert!(block_allocator.check_consistent().is_ok());

        Ok(block_allocator)
    }

    fn mark_used(&mut self, lba: LBA) -> Result<(), ()> {
        let block_index = self
            .free
            .iter()
            .position(|group| group.contains(lba))
            .ok_or(())?;

        let group = &mut self.free[block_index];
        if lba == group.start {
            group.start += 1;
            group.count_minus_one -= 1;
            return Ok(());
        }
        if lba == group.end() {
            group.count_minus_one -= 1;
            return Ok(());
        }

        let end = group.end();
        group.count_minus_one = (lba - group.start) - 1;

        let new_group = BlockGroup::new(lba + 1, end);

        debug_assert!(self.check_consistent().is_ok());

        Ok(())
    }

    /// Allocate a contigious group
    ///
    /// Where possible [Self::allocate] should be used, because it
    /// is better at avoiding fragmentation.
    pub fn allocate_group(&mut self, size: u64) -> Result<BlockGroup, FsError> {
        assert!(size >= 1);
        let mut best: usize = usize::MAX;
        let mut best_rem: u64 = u64::MAX;

        for (i, group) in self.free.iter().enumerate() {
            if group.count() < size {
                continue;
            }

            let rem = group.count() - size;
            if rem < best_rem {
                best_rem = rem;
                best = i;
            }
        }

        if best == usize::MAX {
            return Err(FsError::NoConsecutiveFreeBlocks(size));
        }

        if best_rem == 0 {
            // we can't use swap_remove, because we need to preserve the order
            return Ok(self.free.remove(best));
        }

        let best = &mut self.free[best];
        let result = BlockGroup {
            start: best.start,
            count_minus_one: size - 1,
        };
        best.start += size;
        best.count_minus_one = best_rem - 1;

        debug_assert!(self.check_consistent().is_ok());
        Ok(result)
    }

    /// Allocate `count` [LBA]s
    ///
    /// This tries to reduce fragmentation by prefering [BlockGroup]s
    /// with sizes that are powers of 2.
    pub fn allocate(&mut self, count: u64) -> Result<BlockGroupList, FsError> {
        assert!(count >= 1);

        let mut list = BlockGroupList::new();

        let mut remaining_size = count;
        let mut group_size = 1u64 << count.ilog2() as u64;

        while remaining_size > 0 {
            if group_size == 0 {
                self.free(list);
                return Err(FsError::BlockDeviceFull(count));
            }

            if remaining_size > group_size {
                group_size >>= 1;
                continue;
            }

            let Ok(group) = self.allocate_group(group_size) else {
                // failed to allocate at current size. try smaller sizes
                group_size >>= 1;
                continue;
            };

            list.groups.push(group);
            remaining_size -= group_size;
        }

        assert_eq!(list.block_count(), count);

        Ok(list)
    }

    pub fn free(&mut self, list: BlockGroupList) {
        for group in list.groups {
            self.free_group(group);
        }
    }

    pub fn free_group(&mut self, free: BlockGroup) {
        for i in 0..self.free.len() {
            let current = &mut self.free[i];

            if current.start > free.start {
                self.free.insert(i, free);
                break;
            }

            if current.end() + 1 == free.start {
                current.count_minus_one += free.count();
                break;
            }

            debug_assert!(free.start > current.end());
        }

        debug_assert!(self.check_consistent().is_ok());
    }

    pub fn allocate_block(&mut self) -> Result<LBA, FsError> {
        self.allocate_group(1).map(|group| group.start)
    }

    pub fn free_block(&mut self, block: LBA) {
        self.free_group(BlockGroup {
            start: block,
            count_minus_one: 0,
        });
    }

    /// Check that the internal assumptions of this data structure hold
    ///
    /// # Assumptions
    ///
    /// - free ranges are sorted by their start LBA
    /// - ranges don't overlap
    /// - 2 ranges always have at least 1 LBA between them that is in neither
    #[allow(clippy::result_unit_err)]
    pub fn check_consistent(&self) -> Result<(), ()> {
        if self.free.windows(2).all(|w| {
            assert!(w.len() == 2);
            let first = w[0];
            let second = w[1];

            let past_first_end = first.start + first.count();

            past_first_end < second.start
        }) {
            Ok(())
        } else {
            Err(())
        }
    }

    /// Check that allocator state matches the on device data structures
    ///
    /// This only checks that the free list in the [crate::fs_structs::MainHeader]
    /// matches this allocator.
    /// It does not check that the free list is actually free.
    ///
    /// This will error if the allocator is dirty. See [Self::is_dirty]
    #[allow(clippy::result_unit_err)]
    pub fn check_matches_device<D: BlockDevice>(&self, device: &D) -> Result<(), ()> {
        if self.is_dirty() {
            return Err(());
        }

        self.check_consistent()?;

        todo_warn!("block allocator: check matches device");

        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct BlockGroupList {
    pub groups: Vec<BlockGroup>,
}

impl BlockGroupList {
    pub fn new() -> Self {
        Self { groups: Vec::new() }
    }

    pub fn block_count(&self) -> u64 {
        self.groups.iter().map(|g| g.count()).sum()
    }

    pub fn block_iter(&self) -> BlockGroupListBlockIter<'_> {
        let group_iter = self.groups.iter();
        BlockGroupListBlockIter {
            group_iter,
            current_group: None,
            current_index: 0,
        }
    }
}

pub struct BlockGroupListBlockIter<'a> {
    group_iter: core::slice::Iter<'a, BlockGroup>,
    current_group: Option<BlockGroup>,
    current_index: u64,
}

impl Iterator for BlockGroupListBlockIter<'_> {
    type Item = LBA;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_group.is_none() {
            self.current_group = self.group_iter.next().cloned();
        }
        let group = self.current_group?;

        assert!(self.current_index < group.count());
        let result = group.start + self.current_index;

        if self.current_index == group.count_minus_one {
            self.current_group = None;
            self.current_index = 0;
        } else {
            self.current_index += 1;
        }

        Some(result)
    }
}
