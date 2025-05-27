use core::{
    any::Any,
    assert_matches::assert_matches,
    cmp::min,
    error::{self, Error},
    marker::PhantomData,
    mem::{size_of, transmute},
    ptr::NonNull,
};

use alloc::{
    alloc::AllocError,
    boxed::Box,
    collections::{TryReserveError, TryReserveErrorKind},
    string::{FromUtf8Error, String},
    sync::Arc,
    vec::Vec,
};
use log::{debug, error, warn};
use shared::{counts_required_for, dbg, rangeset::RangeSet, sync::InterruptState, todo_warn};
use static_assertions::const_assert;
use staticvec::StaticVec;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    BLOCK_SIZE, Block, FS_VERSION, LBA,
    block_allocator::{self, BlockAllocator, BlockGroupList},
    existing_fs_check::{FsFound, check_for_filesystem},
    fs_structs::{
        BLOCK_STRING_DATA_LENGTH, BLOCK_STRING_PART_DATA_LENGTH, BlockListHead, BlockString,
        BlockStringPart, DevicePointer, DeviceStringHead, FileId, FileNode, FileType, FsStatus,
        MainHeader, MainTransientHeader, Timestamp, TreeNode,
    },
    interface::BlockDevice,
    mem_structs,
    mem_tree::{self, MemTree, MemTreeError},
};

pub(crate) const MAIN_HEADER_BLOCK: LBA = unsafe { LBA::new_unchecked(0) };
pub(crate) const TREE_ROOT_BLOCK: LBA = unsafe { LBA::new_unchecked(1) };
pub(crate) const FREE_BLOCKS_BLOCK: LBA = unsafe { LBA::new_unchecked(2) };

const INITALLY_USED_BLOCKS: &[LBA] = &[MAIN_HEADER_BLOCK, TREE_ROOT_BLOCK, FREE_BLOCKS_BLOCK];

/// The minimum number of blocks required to create a fs.
///
/// There is no easy formula for this, value. It has to be at least `INITALLY_USED_BLOCKS + 2`
/// (backup header, root directory) however this is just a lower bound.
/// The real value is effected by harder to predict systems like blocks used for
/// the block_allocator.
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
    #[error("MemTree operation failed")]
    MemTreeError(MemTreeError),
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

impl From<TryReserveError> for FsError {
    fn from(value: TryReserveError) -> Self {
        match value.kind() {
            TryReserveErrorKind::CapacityOverflow => panic!(
                "Capacity overflow should never happen, as that would mean that we have an insane size on the device"
            ),
            TryReserveErrorKind::AllocError { .. } => FsError::Oom,
        }
    }
}

pub fn map_device_error<E: Error + Send + Sync + 'static>(e: E) -> FsError {
    FsError::BlockDevice(Box::new(e))
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

pub struct FileSystem<D, S, I> {
    pub device: D,
    max_block_count: u64,
    max_usable_lba: LBA,
    backup_header_lba: LBA,

    pub block_allocator: BlockAllocator,

    header_data: HeaderData,

    mem_tree: MemTree<I>,

    access_mode: AccessMode,

    _state: PhantomData<S>,
}

/// Data from the header about the fs that generally does not change
#[derive(Debug)]
pub struct HeaderData {
    pub name: Option<Box<str>>,
    pub version: [u8; 4],
    pub uuid: Uuid,

    root_ptr: DevicePointer<TreeNode>,
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
        match self.access_mode {
            AccessMode::ReadOnly => {
                // TODO I want to call assert_valid here but I don't want this to panic
                // self.mem_tree.assert_valid()
            }
            AccessMode::ReadWrite => self
                .mem_tree
                .flush_to_device(&mut self.device, &mut self.block_allocator)?,
        }

        // NOTE: should be done last as flushing other data will most likely update the allocator
        if self.block_allocator.is_dirty() {
            assert_matches!(self.access_mode, AccessMode::ReadWrite);
            self.block_allocator.write(&mut self.device)?;
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

        Ok(())
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
                dbg!(closed_header).block_data(),
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
                version: FS_VERSION,
                uuid: Uuid::nil(),
                root_ptr: DevicePointer::new(TREE_ROOT_BLOCK),
            },

            mem_tree,

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
        let free_blocks = FREE_BLOCKS_BLOCK;
        let mut header = Block::new(MainHeader {
            magic: MainHeader::MAGIC,
            version: FS_VERSION,
            uuid,
            root: DevicePointer::new(root_block),
            free_blocks: DevicePointer::new(free_blocks),
            backup_header: DevicePointer::new(fs.backup_header_lba),
            name: None,
            transient: Some(MainTransientHeader {
                magic: MainTransientHeader::MAGIC,
                mount_count: 1,
                open_in_write_mode: true,
                status: FsStatus::Uninitialized,
            }),
        });
        // Write inital header to device to mark fs blocks
        // TODO: should this be a compare_swap? Otherwise there is a possible race
        // between multiple create_internal calls
        fs.write_header(&header)?;
        fs.copy_header_to_backup(&header)?;

        let root_dir = mem_structs::Directory::empty();
        let root_dir_block = root_dir.store(&mut fs)?.lba;
        let root_dir_node = Box::try_new(FileNode {
            id: FileId::ROOT,
            parent: None,
            typ: FileType::Directory,
            permissions: Default::default(),
            _unused: Default::default(),
            uid: 0,
            gid: 0,
            size: 0,
            created_at: Timestamp::zero(),
            modified_at: Timestamp::zero(),
            block_data: root_dir_block.into(),
        })?;

        fs.mem_tree
            .insert(&mut fs.device, root_dir_node.into(), true)?;

        // Flush writes block allocator and initial mem_tree to device
        fs.flush()?;

        let name_block: Option<LBA> = name
            .as_ref()
            .map(|name| fs.write_string(name))
            .transpose()?;
        if let Some(name_block) = name_block {
            header.name = Some(DevicePointer::new(name_block));
        }

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
        fs.write_header(&header)?;
        fs.header_data = HeaderData {
            name,
            version: FS_VERSION,
            uuid,
            root_ptr: DevicePointer::new(TREE_ROOT_BLOCK),
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
            version: header.version,
            uuid: header.uuid,
            root_ptr: header.root,
        };

        let new_header = if force_open {
            warn!("Skip already mounted check");

            let mut new_header = header.clone();
            let new_transient = new_header.transient.as_mut().unwrap();

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

        match fs
            .device
            .compare_exchange_block(
                MAIN_HEADER_BLOCK,
                header.block_data(),
                dbg!(new_header).block_data(),
            )
            .map_err(map_device_error)?
        {
            Ok(()) => {}
            Err(_) => return Err(FsError::CompareExchangeFailedToOften), // TODO loop a few times
        }

        fs.block_allocator =
            BlockAllocator::load(&fs.device, header.free_blocks).map_err(map_device_error)?;
        fs.mem_tree.set_root_device_ptr(header.root);

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

    fn write_header(&mut self, header: &Block<MainHeader>) -> Result<(), FsError> {
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

impl<D: BlockDevice, S: FsRead, I: InterruptState> FileSystem<D, S, I> {
    pub fn read_file_attr(&self, id: FileId) -> Result<Option<Arc<FileNode>>, FsError> {
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
}

impl<D: BlockDevice, S: FsWrite, I> FileSystem<D, S, I> {
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
            string_part.data[..part_data.len()].copy_from_slice(part_data);
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
