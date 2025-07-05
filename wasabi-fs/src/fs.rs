use core::{
    any::Any,
    assert_matches::assert_matches,
    cmp::min,
    error::{self, Error},
    marker::PhantomData,
    mem::{self, size_of, transmute},
    num::NonZeroU64,
    ptr::NonNull,
    usize,
};

use alloc::{
    alloc::AllocError,
    borrow::Cow,
    boxed::Box,
    string::{FromUtf8Error, String},
    sync::Arc,
    vec::Vec,
};
use hashbrown::HashMap;
use log::{debug, error, info, trace, warn};
use shared::{
    alloc_ext::owned_slice::OwnedSlice,
    counts_required_for,
    rangeset::RangeSet,
    sync::{
        InterruptState,
        lockcell::{RWLockCell, ReadWriteCell},
    },
    todo_warn,
};
use static_assertions::const_assert;
use staticvec::StaticVec;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    BLOCK_SIZE, Block, BlockGroup, BlockSlice, FS_VERSION, LBA,
    block_allocator::{self, BlockAllocator, BlockGroupList},
    blocks_required_for,
    existing_fs_check::{FsFound, check_for_filesystem},
    fs_structs::{
        self, BLOCK_STRING_DATA_LENGTH, BLOCK_STRING_PART_DATA_LENGTH, BlockListHead, BlockString,
        BlockStringPart, DevicePointer, DeviceStringHead, FileId, FileNode as FsFileNode, FileType,
        FreeBlockGroups, FsStatus, MainHeader, MainTransientHeader, Perm, Timestamp, TreeNode,
    },
    interface::{BlockDevice, BlockDeviceOrMemError, WriteData},
    mem_structs::{self, BlockList, Directory, DirectoryChange, DirectoryEntry, FileNode},
    mem_tree::{self, MemTree, MemTreeError},
};

pub(crate) const MAIN_HEADER_BLOCK: LBA = unsafe { LBA::new_unchecked(0) };
pub(crate) const TREE_ROOT_BLOCK: LBA = unsafe { LBA::new_unchecked(1) };

const INITALLY_USED_BLOCKS: &[LBA] = &[MAIN_HEADER_BLOCK, TREE_ROOT_BLOCK];

/// The minimum number of blocks required to create a fs.
///
/// There is no easy formula for this, value. It has to be at least `INITALLY_USED_BLOCKS + 2`
/// (backup header, root directory) however this is just a lower bound.
/// The real value is effected by harder to predict systems like blocks used for
/// the block_allocator.
// TODO figure out new min
const MIN_BLOCK_COUNT: u64 = 9;

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum FsError {
    #[error("Block device error: {0}")]
    BlockDevice(Box<dyn Error + Send + Sync>),
    #[error("Block device too small. Max block count is {0}, required: {1}")]
    BlockDeviceToSmall(u64, u64),
    #[error("Block device is full. Failed to allocate {0} blocks")]
    BlockDeviceFull(u64),
    #[error("Failed to find {0} consecutive free blocks")]
    NoConsecutiveFreeBlocks(u64),
    #[error("Override check failed")]
    OverrideCheck,
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
    #[error("Malformed string encountered. String did not match specified length")]
    MalformedStringLength,
    #[error("Malformed string encountered. String must be utf-8 encoded: {0:?}")]
    MalformedStringUtf8(#[from] FromUtf8Error),
    #[error("String length must fit within a u32")]
    StringToLong,
    #[error("Out of Memory")]
    Oom,
    #[error("MemTree operation failed: {0}")]
    MemTreeError(MemTreeError),
    #[error("The requested file({0:?}) does not exist")]
    FileDoesNotExist(FileId),
    #[error("Expected FileId {id} to be a {expected:?}. It is a {file_type:?}")]
    FileTypeMismatch {
        id: FileId,
        file_type: fs_structs::FileType,
        expected: fs_structs::FileType,
    },
    #[error("Read size of 0 bytes is not allowed")]
    ReadZeroBytes,
}

impl From<MemTreeError> for FsError {
    fn from(value: MemTreeError) -> Self {
        match value {
            MemTreeError::Oom(_) => FsError::Oom,
            MemTreeError::BlockDevice(err) => FsError::BlockDevice(err),
            err => FsError::MemTreeError(err),
        }
    }
}

impl From<AllocError> for FsError {
    fn from(value: AllocError) -> Self {
        FsError::Oom
    }
}

impl From<alloc::collections::TryReserveError> for FsError {
    fn from(value: alloc::collections::TryReserveError) -> Self {
        use alloc::collections::TryReserveErrorKind;
        match value.kind() {
            TryReserveErrorKind::CapacityOverflow => panic!(
                "Capacity overflow should never happen, as that would mean that we have an insane size on the device"
            ),
            TryReserveErrorKind::AllocError { .. } => FsError::Oom,
        }
    }
}

impl From<hashbrown::TryReserveError> for FsError {
    fn from(value: hashbrown::TryReserveError) -> Self {
        use hashbrown::TryReserveError;
        match value {
            TryReserveError::CapacityOverflow => panic!(
                "Capacity overflow should never happen, as that would mean that we have an insane size on the device"
            ),
            TryReserveError::AllocError { .. } => FsError::Oom,
        }
    }
}

impl<E: Error + Send + Sync + 'static> From<BlockDeviceOrMemError<E>> for FsError {
    fn from(value: BlockDeviceOrMemError<E>) -> Self {
        match value {
            BlockDeviceOrMemError::BlockDevice(err) => map_device_error(err),
            BlockDeviceOrMemError::Allocation => FsError::Oom,
        }
    }
}

pub fn map_device_error<E: Error + Send + Sync + 'static>(e: E) -> FsError {
    FsError::BlockDevice(Box::new(e))
}

pub trait FsWrite {}

enum FsDuringCreation {}
impl FsWrite for FsDuringCreation {}

pub enum FsReadOnly {}

pub enum FsReadWrite {}
impl FsWrite for FsReadWrite {}

pub struct FileSystem<D, S, I> {
    pub device: D,
    max_block_count: u64,
    max_usable_lba: LBA,
    backup_header_lba: LBA,

    pub block_allocator: BlockAllocator,

    header_data: HeaderData,

    pub mem_tree: MemTree<I>,

    directory_changes: Vec<DirectoryChange>,

    access_mode: AccessMode,

    _state: PhantomData<S>,
}

/// Data from the header about the fs that generally does not change
#[derive(Debug)]
pub struct HeaderData {
    pub name: Option<Box<str>>,
    pub name_head: Option<DevicePointer<BlockString>>,
    pub version: [u8; 4],
    pub uuid: Uuid,

    pub next_file_id: FileId,
    pub dirty: bool,

    status: FsStatus,
    mount_count: u8,

    root_ptr: DevicePointer<TreeNode>,
    free_blocks: Option<DevicePointer<FreeBlockGroups>>,
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

impl<D, I> FileSystem<D, FsDuringCreation, I> {
    /// Converts the state to any finished FsState
    ///
    /// # Safety
    ///
    /// the caller must ensure that the Fs is ready to use
    unsafe fn created<S>(self) -> FileSystem<D, S, I> {
        FileSystem {
            device: self.device,

            max_block_count: self.max_block_count,
            max_usable_lba: self.max_usable_lba,
            backup_header_lba: self.backup_header_lba,

            block_allocator: self.block_allocator,

            header_data: self.header_data,

            mem_tree: self.mem_tree,
            directory_changes: Vec::new(),

            access_mode: self.access_mode,

            _state: PhantomData,
        }
    }
}

impl<D, S, I> FileSystem<D, S, I>
where
    D: BlockDevice,
    I: InterruptState,
{
    /// Ensures that all data is written to the device
    ///
    /// In readonly filesystem this only checks for data consistency
    pub fn flush(&mut self) -> Result<(), FsError> {
        trace!("flush start");
        match self.access_mode {
            AccessMode::ReadOnly => {
                assert!(self.directory_changes.is_empty())
                // TODO I want to call assert_valid here but I don't want this to panic
                // self.mem_tree.assert_valid()
            }
            AccessMode::ReadWrite => {
                let write_fs = self.assume_writable();
                let changes = mem::replace(&mut write_fs.directory_changes, Vec::new());
                write_fs.apply_directory_changes(changes)?;

                self.mem_tree
                    .flush_to_device(&mut self.device, &mut self.block_allocator)?
            }
        }

        // NOTE: should be done last as flushing other data will most likely update the allocator
        if self.block_allocator.is_dirty() || self.header_data.free_blocks.is_none() {
            assert_matches!(
                self.access_mode,
                AccessMode::ReadWrite,
                "Block allocator is dirt({}) or newly crated({}) but fs is readonly",
                self.block_allocator.is_dirty(),
                self.header_data().free_blocks.is_none()
            );
            let free_block_ptr = self.block_allocator.write(&mut self.device)?;
            self.header_data.free_blocks = Some(free_block_ptr);
        } else {
            // TODO disable check outside of debug builds
            if self
                .block_allocator
                .check_matches_device(&self.device)
                .is_err()
            {
                error!("That allocator said it is not dirty, but this is wrong!");
                assert_matches!(self.access_mode, AccessMode::ReadWrite);
                self.block_allocator.write(&mut self.device)?;
            }
        }

        self.write_header()?;

        info!("fs flush done!");
        Ok(())
    }

    /// Fakes write access
    ///
    /// TODO: How do I create a FS that is readonly from a RW fs?
    /// Functions like close and flush should not be allowed for those fs? How would that interact
    /// with Drop
    ///
    /// # Panics
    ///
    /// panics if [Self::access_mode] is not write
    fn assume_writable(&mut self) -> &mut FileSystem<D, FsReadWrite, I> {
        assert_matches!(self.access_mode, AccessMode::ReadWrite);
        // Safety: Access mode allows for write, therefor we can fake a different access mode
        // generic parameter
        unsafe { transmute(self) }
    }

    #[allow(clippy::result_large_err)] // TODO can I fix this somehow? Should I use Box/Arc/etc?
    pub fn close(mut self) -> Result<D, (Self, FsError)> {
        // TODO I want a Drop impl but that conflicts with manually closing and returning the
        // device.
        // I need some type to store the device that allows take on Drop only

        if let Err(e) = self.flush() {
            return Err((self, e));
        }

        let open_header = match self.header() {
            Ok(h) => h,
            Err(e) => return Err((self, e)),
        };
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
        assert_eq!(
            open_header.transient.unwrap().mount_count - 1,
            closed_transient.mount_count
        );
        match self
            .device
            .compare_exchange_block(
                MAIN_HEADER_BLOCK,
                open_header.block_data(),
                closed_header.block_data(),
            )
            .map_err(map_device_error)
        {
            Ok(Ok(())) => Ok(self.device),
            Ok(Err(_)) => Err((self, FsError::CompareExchangeFailedToOften)), // TODO loop a few times
            Err(e) => Err((self, e)),
        }
    }

    /// Creates a [FileSystem] that is not fully initialized
    ///
    /// this is can be used to either load a file system from device or initialize a
    /// new fs on the device.
    #[inline]
    fn create_fs_device_access(
        device: D,
        access: AccessMode,
        mem_tree: MemTree<I>,
    ) -> Result<FileSystem<D, FsDuringCreation, I>, FsError> {
        let max_block_count = device.max_block_count().map_err(map_device_error)?;
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

            header_data: HeaderData {
                name: None,
                name_head: None,
                version: FS_VERSION,
                uuid: Uuid::nil(),
                next_file_id: FileId::ROOT,
                root_ptr: DevicePointer::new(TREE_ROOT_BLOCK),
                status: FsStatus::Uninitialized,
                dirty: true,
                mount_count: 1,
                free_blocks: None,
            },

            mem_tree,
            directory_changes: Vec::new(),

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
        // TODO this function is flushing the fs twice? Is that necessary
        let fs_found = check_for_filesystem(&device).map_err(map_device_error)?;
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

        let mut fs = Self::create_fs_device_access(
            device,
            AccessMode::ReadWrite,
            MemTree::empty(Some(DevicePointer::new(TREE_ROOT_BLOCK))),
        )?;

        // create basic header and backup header
        let root_block = TREE_ROOT_BLOCK;
        // we need to specify something before we write the free block data structure
        // and learn the right LBA. `MAX` might be used for the backup header.
        // That said the value should not matter unless creation crashes and the
        // fs is left in an uninitialized state.
        let free_blocks_uninit_lba = LBA::MAX - 1;
        let mut header = Block::new(MainHeader {
            magic: MainHeader::MAGIC,
            version: FS_VERSION,
            uuid,
            root: DevicePointer::new(root_block),
            free_blocks: DevicePointer::new(LBA::MAX - 1),
            backup_header: DevicePointer::new(fs.backup_header_lba),
            name: None,
            transient: Some(MainTransientHeader {
                magic: MainTransientHeader::MAGIC,
                mount_count: 1,
                open_in_write_mode: true,
                status: FsStatus::Uninitialized,
            }),
            next_file_id: FileId::ROOT.next(),
        });

        // Write inital header to device to mark fs blocks
        // TODO: should this be a compare_swap? Otherwise there is a possible race
        // between multiple create_internal calls
        fs.write_main_header(&header)?;
        fs.copy_header_to_backup(&header)?;

        let name_block: Option<LBA> = name
            .as_ref()
            .map(|name| fs.write_string(name))
            .transpose()?;
        if let Some(name_block) = name_block {
            header.name = Some(DevicePointer::new(name_block));
        }

        let root_dir = mem_structs::Directory::ROOT;
        let root_dir_block = root_dir.write(&mut fs)?.lba;

        let root_dir_node = Box::try_new(FileNode::new(
            FileId::ROOT,
            None,
            FileType::Directory,
            0,
            root_dir_block.into(),
            0,
        ))?;

        fs.mem_tree.create(&mut fs.device, root_dir_node.into())?;

        // Flush writes block allocator and initial mem_tree to device
        fs.flush()?;

        // update transient data in header
        let Some(transient) = header.transient.as_mut() else {
            unreachable!("main header always contains transient header");
        };
        match access {
            AccessMode::ReadOnly => {
                transient.open_in_write_mode = false;
            }
            AccessMode::ReadWrite => {
                transient.open_in_write_mode = true;
            }
        }
        transient.status = FsStatus::Ready;

        // write backup and main header
        // NOTE this must be the last device write before fs.created
        // is called. Otherwise we might leave the FS in an invalid/uninitalized
        // state after setting the status to Ready.
        fs.copy_header_to_backup(&header)?;
        fs.write_main_header(&header)?;
        fs.header_data = HeaderData {
            name,
            name_head: name_block.map(DevicePointer::new),
            version: FS_VERSION,
            uuid,
            next_file_id: header.next_file_id,
            root_ptr: DevicePointer::new(TREE_ROOT_BLOCK),
            status: FsStatus::Ready,
            dirty: false,
            mount_count: 1,
            free_blocks: None,
        };

        unsafe {
            // Safety: we just created an initialized the file system
            Ok(fs.created())
        }
    }

    fn open_internal(device: D, access: AccessMode, force_open: bool) -> Result<Self, FsError> {
        let mut fs = Self::create_fs_device_access(device, access, MemTree::invalid())?;

        let header = fs.header()?;

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

        let block_device_max_block_count = fs.device.max_block_count().map_err(map_device_error)?;
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
                .map_err(map_device_error)?
        };

        if !header.matches_backup(&backup_header) {
            return Err(FsError::HeaderMismatch);
        }

        let name = header.name.map(|name| fs.read_string(name)).transpose()?;

        fs.header_data = HeaderData {
            name,
            name_head: header.name,
            version: header.version,
            uuid: header.uuid,
            root_ptr: header.root,
            next_file_id: header.next_file_id,
            dirty: false,
            // later read from transient header
            status: FsStatus::Uninitialized,
            mount_count: 0,
            free_blocks: Some(header.free_blocks),
        };

        let new_header = if force_open {
            warn!("Skip already mounted check");

            let mut new_header = header.clone();
            let new_transient = new_header.transient.as_mut().unwrap();

            fs.header_data.status = new_transient.status;
            fs.header_data.mount_count = new_transient.mount_count;

            match access {
                AccessMode::ReadOnly => {
                    new_transient.open_in_write_mode = false;
                    new_transient.mount_count = 1;
                }
                AccessMode::ReadWrite => {
                    new_transient.open_in_write_mode = true;
                    new_transient.mount_count = 1;
                }
            }
            new_header
        } else {
            let on_device_transient = header.transient.as_ref().unwrap();

            let mut new_header = header.clone();
            let new_transient = new_header.transient.as_mut().unwrap();

            fs.header_data.status = new_transient.status;
            fs.header_data.mount_count = new_transient.mount_count;

            match access {
                AccessMode::ReadOnly => {
                    if on_device_transient.open_in_write_mode {
                        return Err(FsError::AlreadyMounted);
                    }
                    new_transient.mount_count += 1;
                }
                AccessMode::ReadWrite => {
                    if on_device_transient.mount_count != 0 {
                        return Err(FsError::AlreadyMounted);
                    }
                    new_transient.mount_count += 1;
                    new_transient.open_in_write_mode = true;
                }
            }
            new_header
        };

        let block = new_header;
        match fs
            .device
            .compare_exchange_block(MAIN_HEADER_BLOCK, header.block_data(), block.block_data())
            .map_err(map_device_error)?
        {
            Ok(()) => {}
            Err(_) => return Err(FsError::CompareExchangeFailedToOften), // TODO loop a few times
        }

        fs.block_allocator =
            BlockAllocator::load(&fs.device, header.free_blocks).map_err(map_device_error)?;
        fs.mem_tree.set_root_device_ptr(header.root);

        debug!("header: {header:#?}");
        debug!("free: {:#?}", fs.block_allocator);

        unsafe {
            // Safety: we checked that the fs is valid, and ensured that read/write access is ok
            Ok(fs.created())
        }
    }

    /// Reads the [MainHeader] from [MAIN_HEADER_BLOCK]
    fn header(&self) -> Result<Box<Block<MainHeader>>, FsError> {
        let data = self
            .device
            .read_block(MAIN_HEADER_BLOCK)
            .map_err(map_device_error)?;

        let data: Box<Block<MainHeader>> = unsafe {
            // Safety: Block<MainHeader> is exactly 1 block in size and can be copy constructed.
            // We just read 1 block, which _should_ contain the header
            transmute(data)
        };

        debug_assert!(data.magic == MainHeader::MAGIC);

        Ok(data)
    }

    fn write_header(&mut self) -> Result<(), FsError> {
        trace!("write header");
        // TODO I need a better way to reproduce the header for updates
        let header = Block::new(MainHeader {
            magic: MainHeader::MAGIC,
            version: FS_VERSION,
            uuid: self.header_data.uuid,
            root: DevicePointer::new(TREE_ROOT_BLOCK),
            free_blocks: self
                .header_data
                .free_blocks
                .expect("Free blocks should only be unset during fs creation"),
            backup_header: DevicePointer::new(self.backup_header_lba),
            name: self.header_data.name_head,
            transient: Some(MainTransientHeader {
                magic: MainTransientHeader::MAGIC,
                mount_count: 1,
                open_in_write_mode: self.access_mode == AccessMode::ReadWrite,
                status: self.header_data.status,
            }),
            next_file_id: self.header_data.next_file_id,
        });

        self.copy_header_to_backup(&header)?;
        self.write_main_header(&header)?;

        Ok(())
    }

    fn write_main_header(&mut self, header: &Block<MainHeader>) -> Result<(), FsError> {
        self.device
            .write_block_atomic(MAIN_HEADER_BLOCK, header.block_data())
            .map_err(map_device_error)
    }

    fn copy_header_to_backup(&mut self, header: &Block<MainHeader>) -> Result<(), FsError> {
        let mut backup_header = Block::new(header.clone());
        backup_header.transient = None;
        backup_header.backup_header = DevicePointer::new(MAIN_HEADER_BLOCK);
        self.device
            .write_block_atomic(self.backup_header_lba, backup_header.block_data())
            .map_err(map_device_error)
    }
}

impl<D: BlockDevice, S, I: InterruptState> FileSystem<D, S, I> {
    pub fn read_file_attr(&self, id: FileId) -> Result<Option<Arc<FileNode<I>>>, FsError> {
        self.mem_tree
            .find(&self.device, id)
            .map_err(map_device_error)
    }

    pub fn header_data(&self) -> &HeaderData {
        &self.header_data
    }

    pub fn read_header(&self) -> Result<Box<Block<MainHeader>>, FsError> {
        self.header()
    }

    pub fn read_string_head<const N: usize>(
        &self,
        head: &DeviceStringHead<N>,
    ) -> Result<Box<str>, FsError> {
        let mut string: Vec<u8> = Vec::with_capacity(head.length.to_native() as usize);

        let mut length_remaining = head.length.to_native() as usize;
        let mut next_ptr = head.next;

        let length_in_block = min(length_remaining, BLOCK_STRING_DATA_LENGTH);
        string.extend(&head.data[..length_in_block]);
        length_remaining -= length_in_block;

        while length_remaining > 0 {
            let block = unsafe {
                // We are reading a string
                self.device
                    .read_pointer(next_ptr.ok_or(FsError::MalformedStringLength)?)
            }
            .map_err(map_device_error)?;
            next_ptr = block.next;

            let length_in_block = min(length_remaining, BLOCK_STRING_PART_DATA_LENGTH);

            string.extend(&block.data[..length_in_block]);
            length_remaining -= length_in_block;
        }

        Ok(String::from_utf8(string)?.into_boxed_str())
    }

    pub fn read_string(&self, string_ptr: DevicePointer<BlockString>) -> Result<Box<str>, FsError> {
        let head_block = unsafe {
            // We are reading a string
            self.device.read_pointer(string_ptr)
        }
        .map_err(map_device_error)?;
        self.read_string_head(&head_block.0)
    }

    pub fn read_directory(&self, id: FileId) -> Result<Directory, FsError> {
        let metadata = self
            .mem_tree
            .find(&self.device, id)?
            .ok_or(FsError::FileDoesNotExist(id))?;

        if metadata.typ != FileType::Directory {
            return Err(FsError::FileTypeMismatch {
                id,
                file_type: metadata.typ,
                expected: FileType::Directory,
            });
        }

        metadata.resolve_block_data(&self.device)?;
        let block_data = metadata.block_data.read();
        let block_data = block_data.get_list();

        assert_eq!(block_data.block_count(), 1);
        let block_group = block_data.single();
        Directory::load(self, DevicePointer::new(block_group.start))
    }

    pub fn read_file(
        &self,
        id: FileId,
        offset: u64,
        size: u64,
    ) -> Result<OwnedSlice<u8, impl AsRef<[u8]>>, FsError> {
        if size == 0 {
            return Err(FsError::ReadZeroBytes);
        }

        let metadata = self
            .mem_tree
            .find(&self.device, id)?
            .ok_or(FsError::FileDoesNotExist(id))?;

        if metadata.typ != FileType::File {
            return Err(FsError::FileTypeMismatch {
                id,
                file_type: metadata.typ,
                expected: FileType::File,
            });
        }

        metadata.resolve_block_data(&self.device)?;

        let block_data = metadata.block_data.read();
        let block_list = block_data.get_list().iter_partial(offset, size);

        let offset_in_first_block = block_list
            .clone()
            .next()
            .expect("There should be a block for this file")
            .offset;

        let data = self.device.read_blocks(block_list.map(|g| g.group))?;

        let start = offset_in_first_block as usize;
        let end = (offset_in_first_block + size) as usize;

        assert!(data.len() >= end);

        Ok(OwnedSlice::from(data).subrange(start..end))
    }
}

impl<D: BlockDevice, S: FsWrite, I: InterruptState> FileSystem<D, S, I> {
    fn apply_directory_changes(
        &mut self,
        dir_changes: Vec<DirectoryChange>,
    ) -> Result<(), FsError> {
        let mut changed_dirs = HashMap::<FileId, Directory>::new();

        for dir_change in dir_changes {
            match dir_change {
                DirectoryChange::Created { dir_id, dir } => {
                    trace!("apply new dir created {dir_id:?}: {dir:?}");
                    let dir_lba = dir.write(self)?.lba;

                    let dir_node = Arc::try_new(FileNode::new(
                        dir_id,
                        dir.parent_id,
                        FileType::Directory,
                        0,
                        dir_lba.into(),
                        0,
                    ))?;
                    self.mem_tree.create(&self.device, dir_node)?;
                }
                DirectoryChange::InsertFile { dir_id, entry } => {
                    let changed_dir = if let Some(changed_dir) = changed_dirs.get_mut(&dir_id) {
                        changed_dir
                    } else {
                        let dir = self.read_directory(dir_id)?;

                        changed_dirs.try_reserve(1)?;

                        changed_dirs.insert(dir_id, dir);

                        changed_dirs.get_mut(&dir_id).expect("Just inserted")
                    };
                    debug_assert!(changed_dir.entries.iter().all(|e| e.id != entry.id));
                    changed_dir.entries.push(entry);
                }
            }
        }

        for (dir_id, changed_dir) in changed_dirs {
            let Some(file_node) = (self.mem_tree.find(&self.device, dir_id)?) else {
                error!("Trying to apply directory update to non-existent file {dir_id:?}");
                return Err(FsError::FileDoesNotExist(dir_id));
            };
            if !matches!(file_node.typ, FileType::Directory) {
                error!(
                    "Trying to apply directory update to {:?} file {:?}",
                    file_node.typ, dir_id
                );
                return Err(FsError::FileTypeMismatch {
                    id: dir_id,
                    file_type: file_node.typ,
                    expected: FileType::Directory,
                });
            }

            trace!("update dir {dir_id:?}: {changed_dir:?}");

            let new_lba = changed_dir.write(self)?.lba;

            let mut new_node = FileNode::clone(&file_node);
            new_node.block_data = ReadWriteCell::new(new_lba.into());

            let new_node = Arc::try_new(new_node)?;

            self.mem_tree.update(&self.device, new_node)?;
        }

        Ok(())
    }

    pub fn get_and_inc_file_id(&mut self) -> FileId {
        let next = self.header_data.next_file_id;
        self.header_data.next_file_id = self.header_data.next_file_id.next();
        next
    }

    fn write_tree_node(&mut self, lba: LBA, node: &Block<TreeNode>) -> Result<(), FsError> {
        self.device
            .write_block(lba, node.block_data())
            .map_err(map_device_error)
    }

    fn write_string_parts(
        &mut self,
        parts_substring: &[u8],
        part_blocks: impl Iterator<Item = LBA>,
    ) -> Result<DevicePointer<BlockStringPart>, FsError> {
        let mut bytes = parts_substring;
        let mut blocks = part_blocks.peekable();

        let first_part_lba = *blocks
            .peek()
            .expect("This should never be called with 0 blocks");

        while !bytes.is_empty() {
            let part_lba = blocks
                .next()
                .expect("There should be enough blocks for the string");
            let mut string_part = Block::new(BlockStringPart {
                data: [0; BLOCK_STRING_PART_DATA_LENGTH],
                next: blocks.peek().map(|lba| DevicePointer::new(*lba)),
            });
            let part_data = bytes
                .split_off(..min(BLOCK_STRING_PART_DATA_LENGTH, bytes.len()))
                .expect("we take at max the remaining length");
            string_part.data.data[..part_data.len()].copy_from_slice(part_data);
            self.device
                .write_block(part_lba, string_part.block_data())
                .map_err(map_device_error)?;
        }
        assert!(blocks.next().is_none());

        Ok(DevicePointer::new(first_part_lba))
    }

    pub fn write_string_head<const N: usize>(
        &mut self,
        head: &mut DeviceStringHead<N>,
        string: &str,
    ) -> Result<(), FsError> {
        let string = string.as_bytes();
        let (length_in_parts, length_in_head) = if string.len() > BLOCK_STRING_DATA_LENGTH {
            (
                string.len() - BLOCK_STRING_DATA_LENGTH,
                BLOCK_STRING_DATA_LENGTH,
            )
        } else {
            (0, string.len())
        };

        head.length = TryInto::<u32>::try_into(string.len())
            .map_err(|_| FsError::StringToLong)?
            .into();
        head.data = [0; N];
        let head_slice = &string[0..length_in_head];
        head.data[0..head_slice.len()].copy_from_slice(head_slice);

        if length_in_parts == 0 {
            head.next = None;
            return Ok(());
        }

        let part_substr = &string[N..];
        let part_block_count =
            counts_required_for!(BLOCK_STRING_PART_DATA_LENGTH, length_in_parts) as u64;

        let blocks = self.block_allocator.allocate(part_block_count)?;

        let next = self.write_string_parts(part_substr, blocks.block_iter())?;
        // TODO on error in string_parts I need to free the blocks
        head.next = Some(next);

        Ok(())
    }

    /// Writes a string to the device
    ///
    /// returns the [LBA] of the first block in the [BlockGroupList]
    /// that contains the string
    pub fn write_string(&mut self, string: &str) -> Result<LBA, FsError> {
        let string = string.as_bytes();
        let (length_in_parts, length_in_head) = if string.len() > BLOCK_STRING_DATA_LENGTH {
            (
                string.len() - BLOCK_STRING_DATA_LENGTH,
                BLOCK_STRING_DATA_LENGTH,
            )
        } else {
            (0, string.len())
        };

        let part_block_count =
            counts_required_for!(BLOCK_STRING_PART_DATA_LENGTH, length_in_parts) as u64;

        let blocks = self.block_allocator.allocate(part_block_count + 1)?;

        let mut blocks = blocks.block_iter();

        let head_lba = blocks.next().expect("just allocated at least 1 block");

        let mut head = Block::new(BlockString(DeviceStringHead {
            length: TryInto::<u32>::try_into(string.len())
                .map_err(|_| FsError::StringToLong)?
                .into(),
            data: [0; BLOCK_STRING_DATA_LENGTH],
            next: None,
        }));
        let head_slice = &string[0..length_in_head];
        head.0.data[0..head_slice.len()].copy_from_slice(head_slice);

        if length_in_parts > 0 {
            let next = self.write_string_parts(&string[BLOCK_STRING_DATA_LENGTH..], blocks)?;
            // TODO on error in string_parts I need to free the blocks
            head.0.next = Some(next);
        }

        self.device
            .write_block(head_lba, head.block_data())
            .map_err(map_device_error)?;
        // TODO on error in string_parts I need to free the blocks

        Ok(head_lba)
    }

    pub fn create_directory(&mut self, dir: Directory, name: Box<str>) -> Result<FileId, FsError> {
        trace!("create dir {name}: {dir:?}");
        self.directory_changes.try_reserve(2)?;

        let dir_id = self.get_and_inc_file_id();

        if let Some(parent_id) = dir.parent_id {
            self.directory_changes
                .push_within_capacity(DirectoryChange::InsertFile {
                    dir_id: parent_id,
                    entry: DirectoryEntry { name, id: dir_id },
                })
                .map_err(|_| ())
                .expect("Just allocated additional capacity");
        }

        self.directory_changes
            .push_within_capacity(DirectoryChange::Created { dir_id, dir })
            .map_err(|_| ())
            .expect("Just allocated additional capacity");
        Ok(dir_id)
    }

    pub fn create_file(&mut self, name: Box<str>, parent: FileId) -> Result<FileId, FsError> {
        trace!("create file: name \"{name}\", parent: {parent:?}");

        let Some(parent_dir) = self.mem_tree.find(&self.device, parent)? else {
            return Err(FsError::FileDoesNotExist(parent));
        };

        if !matches!(parent_dir.typ, FileType::Directory) {
            return Err(FsError::FileTypeMismatch {
                id: parent,
                file_type: parent_dir.typ,
                expected: FileType::Directory,
            });
        }

        let file_id = self.get_and_inc_file_id();

        let device_block = self.block_allocator.allocate_block()?;

        // TODO set file permissions
        let file_node = Arc::try_new(FileNode::new(
            file_id,
            Some(parent),
            FileType::File,
            0,
            device_block.into(),
            1,
        ))?;

        self.mem_tree.create(&self.device, file_node)?;
        self.directory_changes.push(DirectoryChange::InsertFile {
            dir_id: parent,
            entry: DirectoryEntry { name, id: file_id },
        });

        Ok(file_id)
    }

    pub fn write_file(&mut self, id: FileId, offset: u64, data: &[u8]) -> Result<(), FsError> {
        let Some(file_node) = self.mem_tree.find(&self.device, id)? else {
            return Err(FsError::FileDoesNotExist(id));
        };

        if !matches!(file_node.typ, FileType::File) {
            return Err(FsError::FileTypeMismatch {
                id,
                file_type: file_node.typ,
                expected: FileType::File,
            });
        }

        if data.is_empty() {
            // warn after we checked the file exists. I still want to error on 0bytes written to
            // non-existent file
            warn!("Writing 0 bytes to file {id:?}");
            return Ok(());
        }
        file_node.resolve_block_data(&self.device)?;

        let mut file_node = Cow::Borrowed(file_node.as_ref());

        let write_len = offset + data.len() as u64;

        if blocks_required_for!(write_len) > file_node.block_count {
            let file_node = file_node.to_mut();

            let blocks_to_alloc = blocks_required_for!(write_len) - file_node.block_count;
            let new_blocks = self.block_allocator.allocate(blocks_to_alloc)?;

            let mut block_list = file_node.block_data.read().get_list().clone();

            block_list.push(new_blocks);

            file_node.block_count = block_list.block_count();
            file_node.block_data = ReadWriteCell::new(block_list.into());
        }

        let keep_last_block_data = if write_len > file_node.size {
            let file_node = file_node.to_mut();
            file_node.size = write_len;
            false
        } else if write_len == file_node.size {
            false
        } else {
            true
        };

        self.write_blocks_partial(
            &file_node.block_data.read().get_list(),
            offset,
            data,
            keep_last_block_data,
        )?;

        if let Cow::Owned(file_node) = file_node {
            let file_node = Arc::try_new(file_node)?;

            self.mem_tree.update(&self.device, file_node)?;
        }

        Ok(())
    }

    fn write_blocks_partial(
        &mut self,
        block_list: &BlockList,
        offset: u64,
        data: &[u8],
        keep_last_block_data: bool,
    ) -> Result<(), FsError> {
        // this should round down, because the offset might start inside of the first block
        let offset_block_skip = offset / BLOCK_SIZE as u64;
        let offset_in_first_block = offset - (offset_block_skip * BLOCK_SIZE as u64);
        let required_block_count = blocks_required_for!(offset_in_first_block + data.len() as u64);

        assert!(block_list.iter().map(|g| g.count()).sum::<u64>() >= required_block_count);

        let groups = block_list.iter_partial(offset, data.len() as u64);

        let old_start_block: Option<Box<BlockSlice>>;
        let old_end_block: Option<Box<BlockSlice>>;

        let mut old_block_start: &[u8] = &[];
        let mut old_block_end: &[u8] = &[];

        assert!(offset_in_first_block < BLOCK_SIZE as u64);

        if offset_in_first_block > 0 {
            let first_group = groups
                .clone()
                .next()
                .expect("There should be at least 1 group");

            assert_eq!(offset_in_first_block, first_group.offset);
            old_start_block = Some(
                self.device
                    .read_block(first_group.group.start)
                    .map_err(map_device_error)?,
            );
            old_block_start = &old_start_block.as_ref().unwrap()[0..offset_in_first_block as usize];
        }
        let last_block_end = (offset_in_first_block + data.len() as u64) % BLOCK_SIZE as u64;
        if keep_last_block_data && last_block_end != 0 {
            let last_group = groups
                .clone()
                .last()
                .expect("There should be at least 1 group");
            let last_block = last_group.group.end();
            old_end_block = Some(
                self.device
                    .read_block(last_block)
                    .map_err(map_device_error)?,
            );

            let last_block_end = (offset_in_first_block + data.len() as u64) % BLOCK_SIZE as u64;

            assert_eq!(last_block_end, last_group.len);

            old_block_end = &old_end_block.as_ref().unwrap()[last_block_end as usize..];
        } else if last_block_end != 0 {
            old_block_end = &Block::ZERO.as_slice()[last_block_end as usize..];
        } else {
            assert_eq!(last_block_end, 0);
            old_block_end = [0u8; 0].as_slice();
        }

        let write_data = WriteData {
            data,
            old_block_start,
            old_block_end,
        };

        self.device
            .write_blocks(groups.map(|g| g.group), write_data)
            .map_err(map_device_error)
    }
}

impl<D: BlockDevice, I: InterruptState> FileSystem<D, FsReadOnly, I> {
    pub fn create(
        device: D,
        override_check: OverrideCheck,
        uuid: Uuid,
        name: Option<Box<str>>,
    ) -> Result<Self, FsError> {
        Self::create_internal(device, uuid, name, override_check, AccessMode::ReadOnly)
    }

    pub fn open(device: D) -> Result<Self, FsError> {
        Self::open_internal(device, AccessMode::ReadOnly, false)
    }
}

impl<D: BlockDevice, I: InterruptState> FileSystem<D, FsReadWrite, I> {
    pub fn create(
        device: D,
        override_check: OverrideCheck,
        uuid: Uuid,
        name: Option<Box<str>>,
    ) -> Result<Self, FsError> {
        Self::create_internal(device, uuid, name, override_check, AccessMode::ReadWrite)
    }

    pub fn open(device: D) -> Result<Self, FsError> {
        Self::open_internal(device, AccessMode::ReadWrite, false)
    }

    pub fn force_open(device: D) -> Result<Self, FsError> {
        Self::open_internal(device, AccessMode::ReadWrite, true)
    }
}
