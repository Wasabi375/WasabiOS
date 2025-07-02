use core::cmp::{max, min};

use crate::block_allocator::BlockAllocator;
use crate::fs::{FsError, FsWrite, map_device_error};
use crate::fs_structs::{
    DIRECTORY_BLOCK_ENTRY_COUNT, Directory as FsDirectory, DirectoryEntry as FsDirectoryEntry,
};
use crate::{BLOCK_SIZE, Block, BlockGroup, block_allocator, blocks_required_for};
use crate::{
    fs_structs::{DevicePointer, FileId},
    interface::BlockDevice,
};
use alloc::{boxed::Box, vec::Vec};
use shared::math::IntoU64;
use shared::sync::InterruptState;
use shared::{counts_required_for, todo_error};
use staticvec::StaticVec;

use super::fs::FileSystem;

#[derive(Debug)]
pub struct Directory {
    pub parent_id: Option<FileId>,
    pub entries: Vec<DirectoryEntry>,
}

impl Directory {
    pub(super) const ROOT: Self = Self {
        parent_id: None,
        entries: Vec::new(),
    };

    pub fn empty(parent_id: FileId) -> Self {
        Self {
            parent_id: Some(parent_id),
            entries: Vec::new(),
        }
    }

    /// Loads a directory from a [BlockDevice]
    pub fn load<D: BlockDevice, S, I: InterruptState>(
        fs: &FileSystem<D, S, I>,
        device_ptr: DevicePointer<FsDirectory>,
    ) -> Result<Self, FsError> {
        let mut next = Some(device_ptr);

        let mut entries = Vec::new();
        let mut expected_rest = 0;

        while let Some(device_ptr) = next {
            let fs_dir = fs
                .device
                .read_pointer(device_ptr)
                .map_err(map_device_error)?;

            if fs_dir.is_head {
                assert_eq!(expected_rest, 0);
                expected_rest = fs_dir.entry_count.to_native();
                entries.try_reserve_exact(expected_rest as usize)?;
            } else {
                assert_eq!(expected_rest, fs_dir.entry_count.to_native());
            }
            expected_rest -= fs_dir.entries.len().into_u64();

            for fs_entry in fs_dir
                .entries
                .iter()
                .map(|entry| DirectoryEntry::load(fs, entry))
            {
                let fs_entry = fs_entry?;
                entries.push(fs_entry);
            }

            next = fs_dir.next;
        }
        assert_eq!(expected_rest, 0);

        todo_error!("directory parent_id is falsely set to `None` during load from device");
        let parent_id = None;

        Ok(Directory { entries, parent_id })
    }

    /// Stores the directory to a [BlockDevice]
    pub fn store<D: BlockDevice, S: FsWrite, I: InterruptState>(
        &self,
        fs: &mut FileSystem<D, S, I>,
    ) -> Result<DevicePointer<FsDirectory>, FsError> {
        let block_count =
            counts_required_for!(DIRECTORY_BLOCK_ENTRY_COUNT, self.entries.len()) as u64;

        if block_count == 0 {
            assert!(self.entries.is_empty());

            let block = fs.block_allocator.allocate_block()?;
            let fs_dir = Block::new(FsDirectory {
                entry_count: 0.into(),
                entries: StaticVec::new(),
                next: None,
                is_head: true,
            });
            fs.device
                .write_block(block, fs_dir.block_data())
                .map_err(map_device_error)?;
            return Ok(DevicePointer::new(block));
        }

        let blocks = fs.block_allocator.allocate(block_count)?;

        let mut fs_dir = Block::new(FsDirectory::default());

        let mut is_head = true;
        let mut entry_count = self.entries.len();
        let mut entries = self.entries.as_slice();

        let mut block_iter = blocks.block_iter().peekable();

        let head_lba = *block_iter.peek().expect("We allocated at least 1 block");

        while let Some(block) = block_iter.next() {
            fs_dir.is_head = is_head;
            is_head = false;

            let block_entry_count = min(DIRECTORY_BLOCK_ENTRY_COUNT, entry_count);
            fs_dir.entry_count = entry_count.into_u64().into();
            entry_count -= block_entry_count;

            let block_entries: &[DirectoryEntry] = entries
                .split_off(..block_entry_count)
                .expect("There should still be block_entry_count entries left");

            fs_dir.entries.clear();

            for entry in block_entries {
                let mut fs_entry = FsDirectoryEntry {
                    name: Default::default(),
                    file_id: entry.id,
                };
                fs.write_string_head(&mut fs_entry.name, &entry.name)?;
                fs_dir.entries.push(fs_entry)
            }

            fs_dir.next = block_iter.peek().map(|lba| DevicePointer::new(*lba));

            fs.device
                .write_block(block, fs_dir.block_data())
                .map_err(map_device_error)?;
        }
        assert_eq!(entry_count, 0);

        Ok(DevicePointer::new(head_lba))
    }
}

#[derive(Debug)]
pub struct DirectoryEntry {
    pub name: Box<str>,
    pub id: FileId,
    // TODO do I want to store the filetype here? At least fuse thinks that readdir should know it
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
}

impl From<BlockGroup> for BlockList {
    fn from(value: BlockGroup) -> Self {
        Self(BlockListInternal::Single([value]))
    }
}

impl From<Vec<BlockGroup>> for BlockList {
    fn from(value: Vec<BlockGroup>) -> Self {
        Self(BlockListInternal::Vec(value))
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
