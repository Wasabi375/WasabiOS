use core::{
    any::Any,
    error::{self, Error},
    marker::PhantomData,
    mem::{size_of, transmute},
    ptr::NonNull,
};

use alloc::boxed::Box;
use shared::{rangeset::RangeSet, todo_error, todo_warn};
use static_assertions::const_assert;
use staticvec::StaticVec;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    fs_structs::{FsStatus, MainHeader, MainTransientHeader, NodePointer, TreeNode},
    interface::BlockDevice,
    mem_structs::BlockAllocator,
    Block, BLOCK_SIZE, FS_VERSION, LBA,
};

const MAIN_HEADER_BLOCK: LBA = unsafe { LBA::new_unchecked(1) };
const ROOT_BLOCK: LBA = unsafe { LBA::new_unchecked(2) };
const FREE_BLOCKS_BLOCK: LBA = unsafe { LBA::new_unchecked(3) };

const INITALLY_USED_BLOCKS: &[LBA] = &[MAIN_HEADER_BLOCK, ROOT_BLOCK, FREE_BLOCKS_BLOCK];

const MIN_BLOCK_COUNT: u64 = 5;

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum FsError {
    #[error("Block device error: {0}")]
    BlockDevice(Box<dyn Error>),
    #[error("Block device too small. Max block count is {0}")]
    BlockDeviceToSmall(u64),
    #[error("Override check failed")]
    OverrideCheck,
    #[error("File system is full")]
    Full,
    #[error("Write allocator failed to allocate blocks for free list")]
    WriteAllocatorFreeList,
}

pub trait FsRead {}
pub trait FsWrite {}

enum FsDuringCreation {}
impl FsRead for FsDuringCreation {}
impl FsWrite for FsDuringCreation {}

pub enum FsReadOnly {}
impl FsRead for FsReadOnly {}

pub enum FsReadWrite {}
impl FsRead for FsReadWrite {}
impl FsWrite for FsReadWrite {}

pub struct FileSystem<D, S> {
    device: D,
    max_block_count: u64,
    max_usable_lba: LBA,
    backup_header_lba: LBA,

    block_allocator: BlockAllocator,

    _state: PhantomData<S>,
}

pub enum OverrideCheck {
    Check,
    IgnoreExisting,
}

enum AccessMode {
    ReadOnly,
    ReadWrite,
}

impl<D> FileSystem<D, FsDuringCreation> {
    /// Converts the state to any finished FsState
    ///
    /// # Safety
    ///
    /// the caller must ensure that the Fs is ready to use
    unsafe fn created<S>(self) -> FileSystem<D, S> {
        FileSystem {
            device: self.device,

            max_block_count: self.max_block_count,
            max_usable_lba: self.max_usable_lba,
            backup_header_lba: self.backup_header_lba,

            block_allocator: self.block_allocator,

            _state: PhantomData,
        }
    }
}

impl<D, S> FileSystem<D, S>
where
    D: BlockDevice,
{
    pub fn close(self) -> D {
        todo_error!("properly close file system");
        self.device
    }

    fn create_fs_device_access(device: D) -> Result<FileSystem<D, FsDuringCreation>, FsError> {
        let max_block_count = device
            .max_block_count()
            .map_err(|e| FsError::BlockDevice(Box::new(e)))?;
        if max_block_count < MIN_BLOCK_COUNT {
            return Err(FsError::BlockDeviceToSmall(max_block_count));
        }
        let max_usable_lba = LBA::new(max_block_count - 2).expect("max_block_count > 2");
        let backup_header_lba = LBA::new(max_block_count - 1).expect("max_block_count > 2");

        Ok(FileSystem {
            device,
            max_block_count,
            max_usable_lba,
            backup_header_lba,

            block_allocator: BlockAllocator::empty(INITALLY_USED_BLOCKS, max_usable_lba),

            _state: PhantomData,
        })
    }

    fn create_internal(
        device: D,
        uuid: Uuid,
        name: Option<&str>,
        _override_check: OverrideCheck,
        access: AccessMode,
    ) -> Result<Self, FsError> {
        todo_warn!("Override check missing");

        let mut fs = Self::create_fs_device_access(device)?;

        // create basic header and backup header
        let root_block = ROOT_BLOCK;
        let free_blocks = FREE_BLOCKS_BLOCK;
        let mut header = Block::new(MainHeader {
            magic: MainHeader::MAGIC,
            version: FS_VERSION,
            uuid,
            root: NodePointer::new(root_block),
            free_blocks: NodePointer::new(free_blocks),
            name: None,
            transient: Some(MainTransientHeader {
                mount_count: 1,
                writeable: true,
                status: FsStatus::Uninitialized,
            }),
        });
        fs.write_header(&header);
        fs.copy_header_to_backup(&header);

        let root = Block::new(TreeNode::Leave {
            parent: None,
            nodes: StaticVec::new(),
        });
        fs.write_tree_node(root_block, &root);

        fs.block_allocator.write(&mut fs.device);

        // TODO create file system name
        let name_block: Option<LBA> = None;
        if let Some(name_block) = name_block {
            header.name = Some(NodePointer::new(name_block));
        }

        // update transient data in header
        let Some(transient) = header.transient.as_mut() else {
            unreachable!("main header always contains transient header");
        };
        transient.status = FsStatus::Ready;
        match access {
            AccessMode::ReadOnly => {
                transient.writeable = false;
            }
            AccessMode::ReadWrite => {}
        }

        // write backup and main header
        fs.copy_header_to_backup(&header);
        fs.write_header(&header);

        unsafe {
            // Safety: we just created an initialized the file system
            Ok(fs.created())
        }
    }
}

impl<D: BlockDevice, S: FsRead> FileSystem<D, S> {
    pub fn header(&self) -> Result<Box<Block<MainHeader>>, FsError> {
        let data = self
            .device
            .read_block(MAIN_HEADER_BLOCK)
            .map_err(|e| FsError::BlockDevice(Box::new(e)))?;

        let data: Box<Block<MainHeader>> = unsafe {
            // Safety: Block<MainHeader> is exactly 1 block in size and can be copy constructed.
            // We just read 1 block, which _should_ contain the header
            transmute(data)
        };

        debug_assert!(data.magic == MainHeader::MAGIC);

        Ok(data)
    }
}
impl<D: BlockDevice, S: FsWrite> FileSystem<D, S> {
    fn write_header(&mut self, header: &Block<MainHeader>) -> Result<(), FsError> {
        self.device
            .write_block_atomic(LBA::new(1).unwrap(), header.block_data())
            .map_err(|e| FsError::BlockDevice(Box::new(e)))
    }

    fn copy_header_to_backup(&mut self, header: &Block<MainHeader>) -> Result<(), FsError> {
        let mut backup_header = Block::new(header.clone());
        backup_header.transient = None;
        self.device
            .write_block_atomic(self.backup_header_lba, backup_header.block_data())
            .map_err(|e| FsError::BlockDevice(Box::new(e)))
    }

    fn write_tree_node(&mut self, lba: LBA, node: &Block<TreeNode>) -> Result<(), FsError> {
        let data: NonNull<[u8; BLOCK_SIZE]> = NonNull::from(node).cast();
        self.device
            .write_block(lba, data)
            .map_err(|e| FsError::BlockDevice(Box::new(e)))
    }
}

impl<D: BlockDevice> FileSystem<D, FsReadOnly> {
    pub fn create(
        device: D,
        override_check: OverrideCheck,
        uuid: Uuid,
        name: Option<&str>,
    ) -> Result<Self, FsError> {
        Self::create_internal(device, uuid, name, override_check, AccessMode::ReadOnly)
    }
}

impl<D: BlockDevice> FileSystem<D, FsReadWrite> {
    pub fn create(
        device: D,
        override_check: OverrideCheck,
        uuid: Uuid,
        name: Option<&str>,
    ) -> Result<Self, FsError> {
        Self::create_internal(device, uuid, name, override_check, AccessMode::ReadWrite)
    }
}
