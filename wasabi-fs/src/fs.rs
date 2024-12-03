use core::{
    any::Any,
    error::{self, Error},
    marker::PhantomData,
    mem::{size_of, transmute},
    ptr::NonNull,
};

use alloc::boxed::Box;
use log::{error, warn};
use shared::{dbg, rangeset::RangeSet, todo_error};
use static_assertions::const_assert;
use staticvec::StaticVec;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    existing_fs_check::{check_for_filesystem, FsFound},
    fs_structs::{FsStatus, MainHeader, MainTransientHeader, NodePointer, TreeNode},
    interface::BlockDevice,
    mem_structs::BlockAllocator,
    Block, BLOCK_SIZE, FS_VERSION, LBA,
};

pub(crate) const MAIN_HEADER_BLOCK: LBA = unsafe { LBA::new_unchecked(0) };
pub(crate) const ROOT_BLOCK: LBA = unsafe { LBA::new_unchecked(1) };
pub(crate) const FREE_BLOCKS_BLOCK: LBA = unsafe { LBA::new_unchecked(2) };

const INITALLY_USED_BLOCKS: &[LBA] = &[MAIN_HEADER_BLOCK, ROOT_BLOCK, FREE_BLOCKS_BLOCK];

// initially used blocks does not have the backup header, as it has a dynamic address
const MIN_BLOCK_COUNT: u64 = INITALLY_USED_BLOCKS.len() as u64 + 1;

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum FsError {
    #[error("Block device error: {0}")]
    BlockDevice(Box<dyn Error>),
    #[error("Block device too small. Max block count is {0}, required: {1}")]
    BlockDeviceToSmall(u64, u64),
    #[error("Override check failed")]
    OverrideCheck,
    #[error("File system is full")]
    Full,
    #[error("Write allocator failed to allocate blocks for free list")]
    WriteAllocatorFreeList,
    #[error("Main header and backup header did not match")]
    HeaderMismatch,
    #[error("Header version is {0:?} but fs is at version {FS_VERSION:?}")]
    HeaderVersionMismatch([u8; 4]),
    #[error("Header did not start with the magic string \"WasabiFs\"")]
    HeaderMagicInvalid,
    #[error("The main header does not include the transient block")]
    HeaderWithoutTransient,
    #[error("The file sytem is not fully initialized")]
    NotInitialized,
    #[error("Failed to compare exchange block to often. Giving up")]
    CompareExchangeFailedToOften,
    #[error("Fs is already mounted by someone else")]
    AlreadyMounted,
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

    header_data: HeaderData,

    access_mode: AccessMode,

    _state: PhantomData<S>,
}

/// Data from the header about the fs that generally does not change
#[derive(Default, Debug)]
pub struct HeaderData {
    pub name: Option<Box<str>>,
    pub version: [u8; 4],
    pub uuid: Uuid,
}

pub enum OverrideCheck {
    Check,
    IgnoreExisting,
}

#[derive(Debug, Clone, Copy, PartialEq)]
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

            header_data: self.header_data,

            access_mode: self.access_mode,

            _state: PhantomData,
        }
    }
}

impl<D, S> FileSystem<D, S>
where
    D: BlockDevice,
{
    pub fn close(mut self) -> Result<D, FsError> {
        if self.block_allocator.is_dirty() {
            self.block_allocator.write(&mut self.device)?;
        } else {
            // TODO disable check outside of debug builds
            if self
                .block_allocator
                .check_matches_device(&self.device)
                .is_err()
            {
                error!("That allocator said it is not dirty, but this is wrong!");
                self.block_allocator.write(&mut self.device)?;
            }
        }

        let open_header = self.header()?;
        let mut closed_header = open_header.clone();
        let closed_transient = &mut closed_header
            .transient
            .as_mut()
            .expect("Main header should always have transient data");
        match self.access_mode {
            AccessMode::ReadOnly => {
                closed_transient.mount_count -= 1;
            }
            AccessMode::ReadWrite => {
                closed_transient.mount_count -= 1;
                closed_transient.open_in_write_mode = false;
            }
        }
        match self
            .device
            .compare_exchange_block(
                MAIN_HEADER_BLOCK,
                open_header.block_data(),
                dbg!(closed_header).block_data(),
            )
            .map_err(|e| FsError::BlockDevice(Box::new(e)))?
        {
            Ok(()) => Ok(self.device),
            Err(_) => Err(FsError::CompareExchangeFailedToOften), // TODO loop a few times
        }
    }

    fn create_fs_device_access(
        device: D,
        access: AccessMode,
    ) -> Result<FileSystem<D, FsDuringCreation>, FsError> {
        let max_block_count = device
            .max_block_count()
            .map_err(|e| FsError::BlockDevice(Box::new(e)))?;
        if max_block_count < MIN_BLOCK_COUNT {
            return Err(FsError::BlockDeviceToSmall(
                max_block_count,
                MIN_BLOCK_COUNT,
            ));
        }
        let max_usable_lba = LBA::new(max_block_count - 2).expect("max_block_count > 2");
        let backup_header_lba = LBA::new(max_block_count - 1).expect("max_block_count > 2");

        Ok(FileSystem {
            device,
            max_block_count,
            max_usable_lba,
            backup_header_lba,

            header_data: Default::default(),

            access_mode: access,

            block_allocator: BlockAllocator::empty(INITALLY_USED_BLOCKS, max_usable_lba),

            _state: PhantomData,
        })
    }

    fn create_internal(
        device: D,
        uuid: Uuid,
        name: Option<Box<str>>,
        override_check: OverrideCheck,
        access: AccessMode,
    ) -> Result<Self, FsError> {
        let fs_found =
            check_for_filesystem(&device).map_err(|e| FsError::BlockDevice(Box::new(e)))?;
        if fs_found != FsFound::None {
            match override_check {
                OverrideCheck::Check => {
                    error!("File system of type {fs_found:?} already exists!");
                    return Err(FsError::OverrideCheck);
                }
                OverrideCheck::IgnoreExisting => {
                    warn!("File system of type {fs_found:?} already exists! It will be overridden");
                }
            }
        }

        let mut fs = Self::create_fs_device_access(device, AccessMode::ReadWrite)?;

        // create basic header and backup header
        let root_block = ROOT_BLOCK;
        let free_blocks = FREE_BLOCKS_BLOCK;
        let mut header = Block::new(MainHeader {
            magic: MainHeader::MAGIC,
            version: FS_VERSION,
            uuid,
            root: NodePointer::new(root_block),
            free_blocks: NodePointer::new(free_blocks),
            backup_header: NodePointer::new(fs.backup_header_lba),
            name: None,
            transient: Some(MainTransientHeader {
                mount_count: 1,
                open_in_write_mode: true,
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
                transient.open_in_write_mode = false;
            }
            AccessMode::ReadWrite => {
                transient.open_in_write_mode = true;
            }
        }

        // write backup and main header
        fs.copy_header_to_backup(&header);
        fs.write_header(&header);
        fs.header_data = HeaderData {
            name,
            version: FS_VERSION,
            uuid,
        };

        unsafe {
            // Safety: we just created an initialized the file system
            Ok(fs.created())
        }
    }

    fn open_internal(device: D, access: AccessMode) -> Result<Self, FsError> {
        let mut fs = Self::create_fs_device_access(device, access)?;

        let header = dbg!(fs.header()?);

        if header.magic != MainHeader::MAGIC {
            return Err(FsError::HeaderMagicInvalid);
        }
        if header.version != FS_VERSION {
            return Err(FsError::HeaderVersionMismatch(header.version));
        }
        if header.transient.is_none() {
            return Err(FsError::HeaderWithoutTransient);
        }
        if header.transient.unwrap().status != FsStatus::Ready {
            return Err(FsError::NotInitialized);
        }

        fs.backup_header_lba = header.backup_header.lba;
        fs.max_usable_lba = fs.backup_header_lba - 1;
        fs.max_block_count = fs.backup_header_lba.get() + 1;

        let block_device_max_block_count = fs
            .device
            .max_block_count()
            .map_err(|e| FsError::BlockDevice(Box::new(e)))?;
        if block_device_max_block_count < fs.max_block_count {
            return Err(FsError::BlockDeviceToSmall(
                block_device_max_block_count,
                fs.max_block_count,
            ));
        }

        let backup_header = unsafe {
            // Safety: we are reading a [MainHeader] from the reported backup location
            fs.device
                .read_pointer(header.backup_header)
                .map_err(|e| FsError::BlockDevice(Box::new(e)))?
        };

        if !header.matches_backup(&backup_header) {
            return Err(FsError::HeaderMismatch);
        }

        fs.header_data = HeaderData {
            name: None, // TODO
            version: header.version,
            uuid: header.uuid,
        };

        let on_device_transient = header.transient.unwrap();
        if on_device_transient.open_in_write_mode {
            return Err(FsError::AlreadyMounted);
        }

        let mut new_header = header.as_ref().clone();
        let new_transient = new_header.transient.as_mut().unwrap();

        match access {
            AccessMode::ReadOnly => {
                new_transient.mount_count += 1;
            }
            AccessMode::ReadWrite => {
                new_transient.mount_count += 1;
                new_transient.open_in_write_mode = true;
            }
        }
        match fs
            .device
            .compare_exchange_block(
                MAIN_HEADER_BLOCK,
                header.block_data(),
                new_header.block_data(),
            )
            .map_err(|e| FsError::BlockDevice(Box::new(e)))?
        {
            Ok(()) => {}
            Err(_) => return Err(FsError::CompareExchangeFailedToOften), // TODO loop a few times
        }

        unsafe {
            // Safety: we checked that the fs is valid, and ensured that read/write access is ok
            Ok(fs.created())
        }
    }

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

    fn write_header(&mut self, header: &Block<MainHeader>) -> Result<(), FsError> {
        self.device
            .write_block_atomic(MAIN_HEADER_BLOCK, header.block_data())
            .map_err(|e| FsError::BlockDevice(Box::new(e)))
    }

    fn copy_header_to_backup(&mut self, header: &Block<MainHeader>) -> Result<(), FsError> {
        let mut backup_header = Block::new(header.clone());
        backup_header.transient = None;
        backup_header.backup_header = NodePointer::new(MAIN_HEADER_BLOCK);
        self.device
            .write_block_atomic(self.backup_header_lba, backup_header.block_data())
            .map_err(|e| FsError::BlockDevice(Box::new(e)))
    }
}

impl<D: BlockDevice, S: FsRead> FileSystem<D, S> {
    pub fn header_data(&self) -> &HeaderData {
        &self.header_data
    }
}

impl<D: BlockDevice, S: FsWrite> FileSystem<D, S> {
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
        name: Option<Box<str>>,
    ) -> Result<Self, FsError> {
        Self::create_internal(device, uuid, name, override_check, AccessMode::ReadOnly)
    }

    pub fn open(device: D) -> Result<Self, FsError> {
        Self::open_internal(device, AccessMode::ReadOnly)
    }
}

impl<D: BlockDevice> FileSystem<D, FsReadWrite> {
    pub fn create(
        device: D,
        override_check: OverrideCheck,
        uuid: Uuid,
        name: Option<Box<str>>,
    ) -> Result<Self, FsError> {
        Self::create_internal(device, uuid, name, override_check, AccessMode::ReadWrite)
    }

    pub fn open(device: D) -> Result<Self, FsError> {
        Self::open_internal(device, AccessMode::ReadWrite)
    }
}
