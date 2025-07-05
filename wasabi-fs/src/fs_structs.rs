use alloc::vec::Vec;
use core::{
    default, fmt,
    marker::PhantomData,
    mem::size_of,
    num::{NonZeroU8, NonZeroU16, NonZeroU64},
    usize,
};

use bitflags::bitflags;
use nonmax::NonMaxU64;
use shared::math::IntoI64;
use simple_endian::LittleEndian;
use static_assertions::{const_assert, const_assert_ne};
use staticvec::{StaticString, StaticVec};
use uuid::Uuid;

use crate::{
    BLOCK_SIZE, BlockGroup, LBA,
    block_allocator::{self, BlockAllocator},
    fs::{FsError, MAIN_HEADER_BLOCK},
    interface::BlockDevice,
    mem_structs,
};

trait BlockLinkedList: Sized + BlockConstructable {
    type Next: BlockLinkedList<Next = Self::Next>;

    /// The pointer to the next part of [Self]
    fn next(&self) -> Option<DevicePointer<Self::Next>>;

    /// Free all blocks within self
    fn free<D: BlockDevice>(
        self: DevicePointer<Self>,
        device: &D,
        block_allocator: &mut BlockAllocator,
    ) -> Result<(), D::BlockDeviceError> {
        let on_device_data = device.read_pointer(self)?;

        if let Some(next) = BlockLinkedList::next(&on_device_data) {
            next.free(device, block_allocator)?;
        }

        block_allocator.free_block(self.lba);

        Ok(())
    }
}

/// A marker trait describing structs that can be constructed from a [super::Block].
///
/// This implies that it is safe to construct the struct from a byte slice read
/// from any block device.
pub trait BlockConstructable {}

/// A pointer of type `T` into a [crate::interface::BlockDevice].
#[derive(Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct DevicePointer<T> {
    pub lba: LBA,
    _block_type: PhantomData<T>,
}
const_assert!(size_of::<DevicePointer<u8>>() == size_of::<LBA>());

impl<T> Clone for DevicePointer<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for DevicePointer<T> {}

impl<T> DevicePointer<T> {
    pub fn new(lba: LBA) -> Self {
        Self {
            lba,
            _block_type: PhantomData,
        }
    }
}

impl<T> core::ops::Receiver for DevicePointer<T> {
    type Target = T;
}

/// Either a single [BlockGroup] or a [DevicePointer] to a [BlockList]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum BlockListHead {
    Single(BlockGroup),
    List(DevicePointer<BlockList>),
}

impl From<LBA> for BlockListHead {
    fn from(value: LBA) -> Self {
        Self::Single(BlockGroup::with_count(value, NonZeroU64::new(1).unwrap()))
    }
}

impl BlockListHead {
    pub fn read<D: BlockDevice>(self, device: &D) -> Result<mem_structs::BlockList, FsError> {
        mem_structs::BlockList::read(self, device)
    }
}

pub const BLOCK_LIST_GROUP_COUNT: usize = (BLOCK_SIZE
    - (size_of::<u8>() + size_of::<DevicePointer<BlockList>>()))
    / size_of::<BlockGroup>();

/// A list of [BlockGroup]s.
///
/// The list is constructed via single linked blocks, that each contain a [StaticVec] of
/// [BlockGroup].
/// When the [StaticVec] is full a new [BlockList] is allocated and linked to in [Self::next]
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct BlockList {
    /// A list of [BlockGroup]s
    pub blocks: StaticVec<BlockGroup, BLOCK_LIST_GROUP_COUNT, u8>,
    /// The next [BlockList] pointer in case this block is not large enough
    pub next: Option<DevicePointer<BlockList>>,
}
const_assert!(size_of::<BlockList>() <= BLOCK_SIZE);
const_assert!(BLOCK_SIZE - size_of::<BlockList>() <= 100);

impl BlockConstructable for BlockList {}

impl BlockLinkedList for BlockList {
    type Next = BlockList;

    fn next(&self) -> Option<DevicePointer<Self::Next>> {
        self.next
    }
}

/// The maximum size of the string data that can be stored in the initial [BlockString].
///
/// The rest of the string data is stored in [BlockStringPart]s
pub(crate) const BLOCK_STRING_DATA_LENGTH: usize =
    BLOCK_SIZE - (size_of::<u32>() + size_of::<Option<DevicePointer<BlockStringPart>>>());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct BlockString(pub DeviceStringHead<BLOCK_STRING_DATA_LENGTH>);
const_assert!(size_of::<BlockString>() == BLOCK_SIZE);

impl BlockConstructable for BlockString {}

impl BlockLinkedList for BlockString {
    type Next = BlockStringPart;

    fn next(&self) -> Option<DevicePointer<Self::Next>> {
        self.0.next
    }
}

/// A string stored over 1 or multiple blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct DeviceStringHead<const N: usize> {
    pub length: LittleEndian<u32>,
    pub data: [u8; N],
    pub next: Option<DevicePointer<BlockStringPart>>,
}

impl<const N: usize> Default for DeviceStringHead<N> {
    fn default() -> Self {
        Self {
            length: 0.into(),
            data: [0; N],
            next: None,
        }
    }
}

/// The maximum size of the string data that can be stored in a [BlockStringPart]
pub(crate) const BLOCK_STRING_PART_DATA_LENGTH: usize =
    BLOCK_SIZE - size_of::<Option<DevicePointer<BlockStringPart>>>();

/// A part of a [BlockString].
///
/// If the initial block of the [BlockString] is not large enough to
/// store all the data, the string is extended by linking one or more
/// [BlockStringPart]s to form a sort of linked list with the remaining data.
/// Only the initial [BlockString] stores the length of the string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct BlockStringPart {
    pub data: [u8; BLOCK_STRING_PART_DATA_LENGTH],
    pub next: Option<DevicePointer<BlockStringPart>>,
}
const_assert!(size_of::<BlockStringPart>() == BLOCK_SIZE);

impl BlockConstructable for BlockStringPart {}

impl BlockLinkedList for BlockStringPart {
    type Next = Self;

    fn next(&self) -> Option<DevicePointer<Self::Next>> {
        self.next
    }
}

/// A unique identifier of a node
///
/// Nodes can be things like files, directories, etc. See [FileType]
///
/// Unique means unique within this filesystem.
/// It is possible for [FileId] to be the same on different file systems
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct FileId(LittleEndian<NonZeroU64>);

impl core::fmt::Debug for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FileId").field(&self.get()).finish()
    }
}

impl FileId {
    pub fn new(id: NonZeroU64) -> Self {
        FileId(id.into())
    }

    pub fn try_new(id: u64) -> Option<Self> {
        NonZeroU64::new(id).map(|id| FileId(id.into()))
    }

    /// # Safety
    ///
    /// `id` must not be `0`
    pub unsafe fn new_unchecked(id: u64) -> Self {
        unsafe { FileId(NonZeroU64::new_unchecked(id).into()) }
    }

    pub fn get(self) -> u64 {
        self.0.to_native().get()
    }

    pub fn next(self) -> Self {
        unsafe { Self::new_unchecked(self.get() + 1) }
    }

    pub const MIN: FileId = FileId::new_const::<1>();

    pub const MAX: FileId = FileId::new_const::<{ u64::MAX }>();

    /// Creates a const file id
    ///
    /// This is the same as `FileId::new(N).unwrap()` but works in a const context.
    /// [Self:new] and [Self::new_unchecked] are not `const` because they rely on
    /// traits that are not yet const stable (rust internal trait attribute).
    pub const fn new_const<const N: u64>() -> Self {
        if N == 0 {
            panic!("FileId::new_const<0>() is an illegal constant");
        }
        FileId(LittleEndian::from_bits(unsafe {
            NonZeroU64::new_unchecked(N.to_le())
        }))
    }

    /// The FileId of the root node
    pub const ROOT: FileId = Self::new_const::<1>();
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.to_native().fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FileType {
    File,
    Directory,
}

/// A timestamp
///
/// Right now this is just a placeholder to reserve memory for future use.
/// It'll probably end up as unix epoch time in ms, but this might change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Timestamp(LittleEndian<u64>);

impl Timestamp {
    pub const fn zero() -> Self {
        Timestamp(LittleEndian::from_bits(0))
    }

    pub fn get(self) -> u64 {
        self.0.into()
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Perm: u8 {
        const EXECUTE = 1 ;
        const WRITE = 1 << 1;
        const READ = 1 << 2;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
// TODO rename FileMeatadata or something
pub struct FileNode {
    pub id: FileId,
    pub parent: Option<FileId>,
    pub typ: FileType,
    pub permissions: [Perm; 4],
    pub _unused: [u8; 3],
    pub uid: u32,
    pub gid: u32,
    /// The size of the file
    ///
    /// Actual value depends on type:
    ///     File: size of the file
    ///     Directory: 0 TODO do I want to store something else here?
    pub size: u64,
    pub created_at: Timestamp,
    pub modified_at: Timestamp, // TODO do I want to differentiate modify and change?
    // TODO do I want last accessed?
    pub block_data: BlockListHead,
    /// The number of blocks used by the FileNode
    ///
    /// Currently this is only set properly for files, and is just 0 for directories
    /// TODO report proper block_count for directories
    pub block_count: u64,
    // TODO do I want some kind of generation counter?
}

impl FileNode {
    pub fn new(
        id: FileId,
        parent: Option<FileId>,
        typ: FileType,
        size: u64,
        block_data: BlockListHead,
        block_count: u64,
    ) -> Self {
        Self {
            id,
            parent,
            typ,
            permissions: [Perm::empty(); 4],
            _unused: [0; 3],
            uid: 0,
            gid: 0,
            size,
            created_at: Timestamp::zero(),
            modified_at: Timestamp::zero(),
            block_data,
            block_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct MainHeader {
    pub magic: [u8; 8],
    /// The version of the file system
    ///
    /// The bytes corespond to major, minor, patch, alpha-status
    ///
    /// ### Alpha-status
    ///
    /// - 255: released
    /// - 0..254: dev, incremented on breaking change during dev.
    pub version: [u8; 4],
    pub uuid: Uuid,
    // _unused1: [u8; 4],
    pub root: DevicePointer<TreeNode>,
    /// Node pointer for the data required for the [crate::block_allocator::BlockAllocator]
    pub free_blocks: DevicePointer<FreeBlockGroups>,
    /// A copy of the header, that should be kept in sync with the main header in the 0th block
    pub backup_header: DevicePointer<MainHeader>,
    /// A name for the filesystem
    // TODO inline that makes writing the header more anoying, because I need to special handle
    // the string part
    pub name: Option<DevicePointer<BlockString>>,
    /// Transient data that describe store the current state of the filesystem.
    ///
    /// In theory this data should not be required on disk, but might be useful for recovery
    /// and error detection.
    // TODO move to the end of header
    pub transient: Option<MainTransientHeader>,

    /// The next free file id.
    ///
    /// FileIds are incremented for now
    pub next_file_id: FileId,
}
const_assert!(size_of::<MainHeader>() <= BLOCK_SIZE);

impl BlockConstructable for MainHeader {}

impl MainHeader {
    /// A magic string that must be part of the header
    pub const MAGIC: [u8; 8] = *b"WasabiFs";

    pub fn matches_backup(&self, backup: &Self) -> bool {
        assert!(self.transient.is_some());
        assert_ne!(self.backup_header.lba, MAIN_HEADER_BLOCK);

        let mut main_copy = self.clone();
        main_copy.backup_header = DevicePointer::new(MAIN_HEADER_BLOCK);
        main_copy.transient = None;

        main_copy == *backup
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum FsStatus {
    /// The file system is not yet initialized
    #[default]
    Uninitialized,
    /// The file system is ready and can be used
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct MainTransientHeader {
    pub magic: [u8; 4],
    pub mount_count: u8,
    pub open_in_write_mode: bool,
    pub status: FsStatus,
}

impl MainTransientHeader {
    /// Magic at the start of the [MainTransientHeader]
    pub const MAGIC: [u8; 4] = *b"WaTh";
}

impl Default for MainTransientHeader {
    fn default() -> Self {
        Self {
            magic: Self::MAGIC,
            mount_count: Default::default(),
            open_in_write_mode: Default::default(),
            status: Default::default(),
        }
    }
}

// NOTE: file and child count are fixed to low values during tests
// in order to make writing unit tests easier.
// 1. I want a low number of branches in the tree to make it easier to construct trees manually
// 2. I want the number of branches to be fixed so as to not break unit tests when adding
//    fields to [FileNode]
pub(crate) const LEAVE_MAX_FILE_COUNT: usize = {
    if cfg!(not(any(test, feature = "test-tree-branching"))) {
        20
    } else {
        6
    }
};
const_assert!(LEAVE_MAX_FILE_COUNT % 2 == 0);

pub(crate) const NODE_MAX_CHILD_COUNT: usize = {
    if cfg!(not(any(test, feature = "test-tree-branching"))) {
        250
    } else {
        30
    }
};
const_assert!(NODE_MAX_CHILD_COUNT % 2 == 0);
// this simplifies rebalance
const_assert!(NODE_MAX_CHILD_COUNT / 2 >= 2);

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C, u8)]
#[allow(clippy::large_enum_variant)]
pub enum TreeNode {
    Leave {
        parent: Option<DevicePointer<TreeNode>>, // TODO do I need the parent pointer?
        files: StaticVec<FileNode, LEAVE_MAX_FILE_COUNT, u8>,
    },
    Node {
        /// The parent of this Node or `None` if this is the root node
        parent: Option<DevicePointer<TreeNode>>, // TODO do I need the parent pointer?
        /// a list of [TreeNode] pointers and their maximum [FileId] value.
        ///
        /// `children[i].0 == children[i].1.follow().max`
        children: StaticVec<(DevicePointer<TreeNode>, Option<FileId>), NODE_MAX_CHILD_COUNT, u8>,
    },
}
const_assert!(size_of::<TreeNode>() <= BLOCK_SIZE);

impl BlockConstructable for TreeNode {}

/// The number of free [BlockGroup]s that fit within a single [FreeBlockGroups]
pub(crate) const BLOCK_RANGES_COUNT_PER_BLOCK: usize = 250;

/// Used to keep track of unsused [BlockGroup]s.
///
/// This should be accessed through [crate::block_allocator::BlockAllocator].
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct FreeBlockGroups {
    /// Unused [BlockGroup]s
    pub free: StaticVec<BlockGroup, BLOCK_RANGES_COUNT_PER_BLOCK, u8>,
    /// A [DevicePointer] to the next [FreeBlockGroups] of unused [BlockGroup]s.
    ///
    /// This might be `Some` even if [Self::free] is not full.
    /// The [crate::block_allocator::BlockAllocator] might uses empty [FreeBlockGroups] as reserved blocks.
    pub next: Option<DevicePointer<FreeBlockGroups>>,
}
const_assert!(size_of::<FreeBlockGroups>() <= BLOCK_SIZE);
const_assert!(BLOCK_SIZE - size_of::<FreeBlockGroups>() <= 100);

impl BlockConstructable for FreeBlockGroups {}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: DeviceStringHead<100>,
    pub file_id: FileId,
}

/// The maximum number of entries within a [Directory] block.
///
/// If a Directory has more entries a new [Directory] block is linked in [Directory::next]
pub(crate) const DIRECTORY_BLOCK_ENTRY_COUNT: usize = 33;
#[repr(C)]
pub struct Directory {
    /// total number of entries within the [Directory]
    pub entry_count: LittleEndian<u64>,
    /// [FileId]s of each [FileNode] within this [Directory]
    pub entries: StaticVec<DirectoryEntry, DIRECTORY_BLOCK_ENTRY_COUNT, u8>,
    /// if [Self::entry_count] is greater than [DIRECTORY_BLOCK_ENTRY_COUNT] this
    /// points to the next [Directory] block
    pub next: Option<DevicePointer<Directory>>,
    /// Set to `true` if this is the first block in the linked list describing the Directory
    pub is_head: bool,
}
const_assert!(size_of::<Directory>() <= BLOCK_SIZE);
const_assert!(BLOCK_SIZE - size_of::<Directory>() <= size_of::<DirectoryEntry>());

impl BlockConstructable for Directory {}

impl Default for Directory {
    fn default() -> Self {
        Self {
            entry_count: 0.into(),
            entries: StaticVec::new(),
            next: None,
            is_head: true,
        }
    }
}

impl BlockLinkedList for Directory {
    type Next = Directory;

    fn next(&self) -> Option<DevicePointer<Self::Next>> {
        self.next
    }
}
