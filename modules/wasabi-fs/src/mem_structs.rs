use core::cmp::{max, min};
use core::sync::atomic::AtomicU64;

use crate::block_allocator::{BlockAllocator, BlockGroupList};
use crate::fs::{FsError, FsMalformedError, FsWrite, map_device_error};
use crate::fs_structs::{
    BLOCK_LIST_GROUP_COUNT, BlockListHead, DIRECTORY_HEAD_ENTRY_COUNT, DIRECTORY_PART_ENTRY_COUNT,
    DirectoryEntry as FsDirectoryEntry, DirectoryHead, DirectoryPart, FileNode as FsFileNode,
    FileType, Perm, Timestamp,
};
use crate::{BLOCK_SIZE, Block, BlockGroup, LBA, block_allocator, blocks_required_for};
use crate::{
    fs_structs::{BlockList as FsBlockList, DevicePointer, FileId},
    interface::BlockDevice,
};
use alloc::sync::Arc;
use alloc::{boxed::Box, vec::Vec};
use log::{error, trace};
use shared::iter::IterExt;
use shared::math::{IntoU64, IntoUSize};
use shared::sync::InterruptState;
use shared::sync::lockcell::{RWLockCell, ReadWriteCell};
use shared::{counts_required_for, todo_error};
use staticvec::StaticVec;

use super::fs::FileSystem;

#[derive(Debug)]
pub struct Directory {
    pub parent_id: Option<FileId>,
    pub entries: Vec<DirectoryEntry>,
}

impl Directory {
    /// Create a [Self] that represents the empty root of a file system
    pub(super) const fn create_root() -> Self {
        Self {
            parent_id: None,
            entries: Vec::new(),
        }
    }

    pub fn empty(parent_id: FileId) -> Self {
        Self {
            parent_id: Some(parent_id),
            entries: Vec::new(),
        }
    }

    /// Loads a directory from a [BlockDevice]
    pub fn read<D: BlockDevice, S, I: InterruptState>(
        fs: &FileSystem<D, S, I>,
        device_ptr: DevicePointer<DirectoryHead>,
    ) -> Result<Self, FsError> {
        fn extend_entries<D: BlockDevice, S, I: InterruptState>(
            fs: &FileSystem<D, S, I>,
            mem_entries: &mut Vec<DirectoryEntry>,
            device_entries: &[FsDirectoryEntry],
        ) -> Result<(), FsError> {
            for entry in device_entries {
                let mem_entry = DirectoryEntry::load(fs, entry)?;
                mem_entries.push(mem_entry);
            }
            Ok(())
        }

        let head = fs
            .device
            .read_pointer(device_ptr)
            .map_err(map_device_error)?;

        let expected_count = head.entry_count.to_native() as usize;

        if expected_count < head.entries.len() as usize {
            return Err(FsMalformedError::DirectoryEntryCountMismatch(
                expected_count,
                head.entries.len() as usize,
            )
            .into());
        }

        let mut entries = Vec::try_with_capacity(expected_count)?;

        extend_entries(fs, &mut entries, &head.entries)?;

        let mut next_part_ptr = head.next;

        while let Some(part_ptr) = next_part_ptr {
            let part = fs.device.read_pointer(part_ptr).map_err(map_device_error)?;

            if entries.len() + part.entries.len() as usize > expected_count {
                return Err(FsMalformedError::DirectoryEntryCountMismatch(
                    expected_count,
                    entries.len() + part.entries.len() as usize,
                )
                .into());
            }

            extend_entries(fs, &mut entries, &part.entries)?;

            next_part_ptr = part.next;
        }

        if entries.len() != expected_count {
            return Err(FsMalformedError::DirectoryEntryCountMismatch(
                expected_count,
                entries.len(),
            )
            .into());
        }

        Ok(Directory {
            parent_id: head.parent,
            entries,
        })
    }

    /// Stores the directory to a [BlockDevice]
    pub fn write<D: BlockDevice, S: FsWrite, I: InterruptState>(
        &self,
        fs: &mut FileSystem<D, S, I>,
    ) -> Result<DevicePointer<DirectoryHead>, FsError> {
        let entries_in_parts = if self.entries.len() > DIRECTORY_HEAD_ENTRY_COUNT {
            self.entries.len() - DIRECTORY_HEAD_ENTRY_COUNT
        } else {
            0
        };

        let blocks_required = blocks_required_for!(entries_in_parts) + 1;

        // TODO free blocks on error
        let blocks = fs.block_allocator.allocate(blocks_required)?;
        let mut blocks = blocks.block_iter().peekable();

        let mut entries = self.entries.as_slice();

        let head_block_lba = blocks.next().expect("Allocated at least 1 block");

        let mut head_entries = StaticVec::new();
        for mem_entry in entries
            .split_off(..min(DIRECTORY_HEAD_ENTRY_COUNT, entries.len()))
            .expect("Take at lest len entries")
        {
            head_entries.push(mem_entry.write(fs)?);
        }
        let head_block = Block::new(DirectoryHead {
            parent: self.parent_id,
            entry_count: (self.entries.len() as u64).into(),
            entries: head_entries,
            next: blocks.peek().map(|lba| DevicePointer::new(*lba)),
        });

        fs.device
            .write_block(head_block_lba, head_block.block_data())
            .map_err(map_device_error)?;

        for ((block_lba, next_lba), entries) in blocks
            .peeked()
            .zip(entries.chunks(DIRECTORY_PART_ENTRY_COUNT))
        {
            let mut part_entries = StaticVec::new();
            for mem_entry in entries {
                part_entries.push(mem_entry.write(fs)?);
            }
            let part_block = Block::new(DirectoryPart {
                entries: part_entries,
                next: next_lba.map(|lba| DevicePointer::new(lba)),
            });

            fs.device
                .write_block(block_lba, part_block.block_data())
                .map_err(map_device_error)?;
        }

        Ok(DevicePointer::new(head_block_lba))
    }
}

#[derive(Debug)]
pub struct DirectoryEntry {
    pub name: Box<str>,
    pub id: FileId,
}

impl DirectoryEntry {
    pub fn load<D: BlockDevice, S, I: InterruptState>(
        fs: &FileSystem<D, S, I>,
        fs_entry: &FsDirectoryEntry,
    ) -> Result<Self, FsError> {
        let name = fs.read_string_head(&fs_entry.name)?;

        Ok(DirectoryEntry {
            name,
            id: fs_entry.file_id,
        })
    }

    pub fn write<D: BlockDevice, S: FsWrite, I: InterruptState>(
        &self,
        fs: &mut FileSystem<D, S, I>,
    ) -> Result<FsDirectoryEntry, FsError> {
        let name = fs.write_string_head(&self.name)?;

        Ok(FsDirectoryEntry {
            name,
            file_id: self.id,
        })
    }
}

pub enum DirectoryChange {
    Created {
        dir_id: FileId,
        dir: Directory,
    },
    // TODO do I want a full overwrite?
    // Updated { dir_id: FileId, new_dir: Directory },
    InsertFile {
        dir_id: FileId,
        entry: DirectoryEntry,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockList(BlockListInternal);

#[derive(Debug, Clone, PartialEq, Eq)]
enum BlockListInternal {
    Single([BlockGroup; 1]),
    Vec(Vec<BlockGroup>),
}

impl BlockList {
    pub fn as_slice(&self) -> &[BlockGroup] {
        match &self.0 {
            BlockListInternal::Single(group) => group.as_slice(),
            BlockListInternal::Vec(groups) => groups.as_slice(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = BlockGroup> + Clone {
        self.as_slice().iter().cloned()
    }

    pub fn iter_partial(
        &self,
        byte_offset: u64,
        bytes: u64,
    ) -> BlockListPartialIter<impl Iterator<Item = BlockGroup> + Clone> {
        BlockListPartialIter {
            group_iter: self.iter(),
            skip_bytes: byte_offset,
            remaining_bytes: bytes,
        }
    }

    pub fn block_count(&self) -> u64 {
        // TODO memorize?
        self.iter().map(|g| g.count()).sum()
    }

    pub fn single(&self) -> BlockGroup {
        assert_eq!(self.as_slice().len(), 1);
        self.as_slice()[0]
    }

    pub fn push(&mut self, group: BlockGroupList) {
        match &mut self.0 {
            BlockListInternal::Single([first]) => {
                let mut vec = Vec::with_capacity(group.groups.len() + 1);
                vec.push(*first);
                vec.extend(group.groups.into_iter());

                *self = vec.into()
            }
            BlockListInternal::Vec(vec) => vec.extend(group.groups.into_iter()),
        }
    }

    pub fn read<D: BlockDevice>(head: BlockListHead, device: &D) -> Result<Self, FsError> {
        let mut next_block_ptr = match head {
            BlockListHead::Single(group) => return Ok(Self(BlockListInternal::Single([group]))),
            BlockListHead::List(ptr) => Some(ptr),
        };

        let mut list = Vec::new();

        while let Some(block_ptr) = next_block_ptr {
            let block = device.read_pointer(block_ptr).map_err(map_device_error)?;

            list.try_reserve(block.blocks.len().into_usize())?;
            list.extend(block.blocks.into_iter());

            next_block_ptr = block.next;
        }
        Ok(list.into())
    }

    pub fn write<D: BlockDevice>(
        &self,
        device: &mut D,
        block_allocator: &mut BlockAllocator,
    ) -> Result<BlockListHead, FsError> {
        let block_groups = match &self.0 {
            BlockListInternal::Vec(block_groups) => block_groups,
            BlockListInternal::Single([group]) => {
                return Ok(BlockListHead::Single(*group));
            }
        };

        let data_block_count = counts_required_for!(BLOCK_LIST_GROUP_COUNT, block_groups.len());
        assert!(data_block_count >= 1);

        let data_blocks = block_allocator.allocate(data_block_count as u64)?;

        let first_block_addr = data_blocks.groups[0].start;

        for ((block_addr, next_block_addr), groups) in data_blocks
            .block_iter()
            .peeked()
            .zip(block_groups.chunks(BLOCK_LIST_GROUP_COUNT))
        {
            let block = Block::new(FsBlockList {
                blocks: StaticVec::from(groups),
                next: next_block_addr.map(|lba| DevicePointer::new(lba)),
            });

            device
                .write_block(block_addr, block.block_data())
                .map_err(map_device_error)?;
        }

        Ok(BlockListHead::List(DevicePointer::new(first_block_addr)))
    }
}

impl From<BlockGroup> for BlockList {
    fn from(value: BlockGroup) -> Self {
        Self(BlockListInternal::Single([value]))
    }
}

impl From<BlockGroupList> for BlockList {
    fn from(value: BlockGroupList) -> Self {
        Self(BlockListInternal::Vec(value.groups))
    }
}

impl From<Vec<BlockGroup>> for BlockList {
    fn from(value: Vec<BlockGroup>) -> Self {
        Self(BlockListInternal::Vec(value))
    }
}

impl From<LBA> for BlockList {
    fn from(value: LBA) -> Self {
        BlockGroup::new(value, value).into()
    }
}

#[derive(Debug, Clone)]
pub enum BlockListAccess {
    OnDevice(BlockListHead),
    InMem(BlockList),
    Both(BlockList, BlockListHead),
}

impl BlockListAccess {
    pub fn get_list(&self) -> &BlockList {
        match self {
            BlockListAccess::OnDevice(_) => panic!("BlockListAccess not resolved to InMem"),
            BlockListAccess::InMem(list) | BlockListAccess::Both(list, _) => list,
        }
    }

    pub fn get_list_mut(&mut self) -> &mut BlockList {
        match self {
            BlockListAccess::OnDevice(_) => panic!("BlockListAccess not resolved to InMem"),
            BlockListAccess::InMem(list) | BlockListAccess::Both(list, _) => list,
        }
    }
}

impl From<BlockListHead> for BlockListAccess {
    fn from(value: BlockListHead) -> Self {
        Self::OnDevice(value)
    }
}

impl From<BlockList> for BlockListAccess {
    fn from(value: BlockList) -> Self {
        Self::InMem(value)
    }
}

impl From<LBA> for BlockListAccess {
    fn from(value: LBA) -> Self {
        BlockListHead::from(value).into()
    }
}

#[derive(Debug, Clone)]
pub struct BlockListPartialIter<I> {
    /// The underlying group iterator
    group_iter: I,
    /// the number of bytes to skip in the underlying iterator
    skip_bytes: u64,
    /// the total remaining number of bytes that should be included in the groups
    remaining_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartialBlockGroup {
    /// BlockGroup
    pub group: BlockGroup,
    /// byte offset into the group
    pub offset: u64,
    /// number of bytes to use within the group
    pub len: u64,
}

impl<I> Iterator for BlockListPartialIter<I>
where
    I: Iterator<Item = BlockGroup>,
{
    type Item = PartialBlockGroup;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(mut current_group) = self.group_iter.next() {
            if self.skip_bytes >= current_group.bytes() {
                self.skip_bytes -= current_group.bytes();
                continue;
            }

            let offset = if self.skip_bytes > 0 {
                let blocks_to_skip = self.skip_bytes / BLOCK_SIZE as u64;
                let offset = self.skip_bytes % BLOCK_SIZE as u64;

                current_group = current_group.subgroup(blocks_to_skip);

                self.skip_bytes = 0;
                offset
            } else {
                0
            };

            let len = if self.remaining_bytes < current_group.bytes() {
                current_group = current_group.shorten(blocks_required_for!(self.remaining_bytes));
                self.remaining_bytes
            } else {
                current_group.bytes()
            };
            assert!(self.remaining_bytes >= len);

            self.remaining_bytes -= len;

            assert!(offset + len <= current_group.bytes());
            return Some(PartialBlockGroup {
                group: current_group,
                offset,
                len,
            });
        }

        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, self.group_iter.size_hint().1)
    }
}

#[derive_where::derive_where(Debug)]
pub struct FileNode<I> {
    pub id: FileId,
    pub parent: Option<FileId>,
    pub typ: FileType,
    pub permissions: [Perm; 4],
    pub uid: u32,
    pub gid: u32,
    /// The size of the file
    ///
    /// See [crate::fs_structs::FileNode::size]
    pub size: u64,
    pub created_at: Timestamp,
    pub modified_at: Timestamp,

    #[derive_where(skip)]
    pub block_data: ReadWriteCell<BlockListAccess, I>,
    /// The number of blocks used by the FileNode
    ///
    /// See [crate::fs_structs::FileNode::block_cont]
    pub block_count: u64,
}

impl<I: InterruptState> Clone for FileNode<I> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            parent: self.parent.clone(),
            typ: self.typ.clone(),
            permissions: self.permissions.clone(),
            uid: self.uid.clone(),
            gid: self.gid.clone(),
            size: self.size.clone(),
            created_at: self.created_at.clone(),
            modified_at: self.modified_at.clone(),
            block_data: ReadWriteCell::new(self.block_data.read().clone()),
            block_count: self.block_count.clone(),
        }
    }
}

impl<I: InterruptState> FileNode<I> {
    pub fn new(
        id: FileId,
        parent: Option<FileId>,
        typ: FileType,
        size: u64,
        block_data: BlockListAccess,
        block_count: u64,
    ) -> Self {
        Self {
            id,
            parent,
            typ,
            permissions: [Perm::empty(); 4],
            uid: 0,
            gid: 0,
            size,
            created_at: Timestamp::zero(),
            modified_at: Timestamp::zero(),
            block_data: ReadWriteCell::new(block_data),
            block_count,
        }
    }

    pub fn write<D: BlockDevice>(
        &self,
        device: &mut D,
        block_allocator: &mut BlockAllocator,
    ) -> Result<FsFileNode, FsError> {
        let mut block_data_guard = self.block_data.write();

        let block_data = match &mut *block_data_guard {
            BlockListAccess::OnDevice(data) => *data,
            BlockListAccess::InMem(list) | BlockListAccess::Both(list, _) => {
                let head = list.write(device, block_allocator)?;

                // temp write a bogus value to the block_data so that I can update the entire
                // access. This is ok, because I have the write gaurd already so no one will be
                // able to read the fake value
                let list = core::mem::replace(list, LBA::MAX.into());
                *block_data_guard = BlockListAccess::Both(list, head);

                head
            }
        };

        Ok(FsFileNode {
            id: self.id,
            parent: self.parent,
            typ: self.typ,
            permissions: self.permissions,
            _unused: [0; 3],
            uid: self.uid,
            gid: self.gid,
            size: self.size,
            created_at: self.created_at,
            modified_at: self.modified_at,
            block_data,
            block_count: self.block_count,
        })
    }

    pub fn resolve_block_data<D: BlockDevice>(&self, device: &D) -> Result<(), FsError> {
        let read_guard = self.block_data.read();
        match &*read_guard {
            BlockListAccess::OnDevice(_) => {
                drop(read_guard);
                let mut block_data = self.block_data.write();
                if let BlockListAccess::OnDevice(head) = &*block_data {
                    let list = head.read(device)?;
                    *block_data = BlockListAccess::InMem(list);
                }
            }
            BlockListAccess::InMem(_) | BlockListAccess::Both(_, _) => {}
        }

        Ok(())
    }
}

impl<I> From<FsFileNode> for FileNode<I> {
    fn from(device_node: FsFileNode) -> Self {
        Self {
            id: device_node.id,
            parent: device_node.parent,
            typ: device_node.typ,
            permissions: device_node.permissions,
            uid: device_node.uid,
            gid: device_node.gid,
            size: device_node.size,
            created_at: device_node.created_at,
            modified_at: device_node.modified_at,
            block_data: ReadWriteCell::new(BlockListAccess::OnDevice(device_node.block_data)),
            block_count: device_node.block_count,
        }
    }
}
