use core::cmp::{max, min};

use crate::block_allocator::BlockAllocator;
use crate::fs::{FsError, FsWrite, map_device_error};
use crate::fs_structs::{
    DIRECTORY_BLOCK_ENTRY_COUNT, Directory as FsDirectory, DirectoryEntry as FsDirectoryEntry,
};
use crate::{Block, BlockGroup, block_allocator, blocks_required_for};
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
