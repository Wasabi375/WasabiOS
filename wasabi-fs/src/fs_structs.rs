use core::{
    default, fmt,
    marker::PhantomData,
    mem::size_of,
    num::{NonZeroU8, NonZeroU16, NonZeroU64},
};

use bitflags::bitflags;
use nonmax::NonMaxU64;
use shared::math::IntoI64;
use simple_endian::LittleEndian;
use static_assertions::const_assert;
use staticvec::{StaticString, StaticVec};
use uuid::Uuid;

use crate::{BLOCK_SIZE, BlockGroup, LBA, fs::MAIN_HEADER_BLOCK};

/// Either a single [BlockGroup] or a [NodePointer] to a [BlockList]
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub enum BlockListHead {
    Single(BlockGroup),
    List(NodePointer<BlockList>),
}

const BLOCK_LIST_GROUP_COUNT: usize =
    (512 - (size_of::<u8>() + size_of::<NodePointer<BlockList>>())) / size_of::<BlockGroup>();

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
    pub next: Option<NodePointer<BlockList>>,
}
const_assert!(size_of::<BlockList>() <= BLOCK_SIZE);

/// The maximum size of the string data that can be stored in the initial [BlockString].
///
/// The rest of the string data is stored in [BlockStringPart]s
pub(crate) const BLOCK_STRING_DATA_LENGTH: usize =
    512 - (size_of::<u32>() + size_of::<Option<NodePointer<BlockStringPart>>>());

/// A string stored over 1 or multiple blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct BlockString {
    pub length: LittleEndian<u32>,
    pub data: [u8; BLOCK_STRING_DATA_LENGTH],
    pub next: Option<NodePointer<BlockStringPart>>,
}
const_assert!(size_of::<BlockString>() == BLOCK_SIZE);

/// The maximum size of the string data that can be stored in a [BlockStringPart]
pub(crate) const BLOCK_STRING_PART_DATA_LENGTH: usize =
    512 - size_of::<Option<NodePointer<BlockStringPart>>>();

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
    pub next: Option<NodePointer<BlockStringPart>>,
}
const_assert!(size_of::<BlockStringPart>() == BLOCK_SIZE);

/// A unique identifier of a node
///
/// Nodes can be things like files, directories, etc. See [FileType]
///
/// Unique means unique within this filesystem.
/// It is possible for [FileId] to be the same on different file systems
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct FileId(LittleEndian<NonMaxU64>);

impl FileId {
    pub fn new(id: NonMaxU64) -> Self {
        FileId(id.into())
    }

    pub fn try_new(id: u64) -> Option<Self> {
        NonMaxU64::new(id).map(|id| FileId(id.into()))
    }

    /// # Safety
    ///
    /// `id` must not be `u64::MAX`
    pub unsafe fn new_unchecked(id: u64) -> Self {
        unsafe { FileId(NonMaxU64::new_unchecked(id).into()) }
    }

    pub fn get(self) -> u64 {
        self.0.to_native().get()
    }

    pub const MIN: FileId = unsafe {
        // Safety: 0 is a valid non-max
        FileId(LittleEndian::from_bits(NonMaxU64::new_unchecked(0)))
    };

    pub const MAX: FileId = unsafe {
        // Safety: MAX - 1 is a valid non-max
        FileId(LittleEndian::from_bits(NonMaxU64::new_unchecked(
            (u64::MAX - 1).to_le(),
        )))
    };
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
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Perm: u8 {
        const EXECUTE = 1 ;
        const WRITE = 1 << 1;
        const READ = 1 << 2;
    }
}

const I_NODE_MAX_NAME_LEN: usize = 40;
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct FileNode {
    pub id: FileId,
    pub parent: Option<FileId>,
    pub typ: FileType,
    pub permissions: [Perm; 4],
    pub _unused: [u8; 3],
    pub size: u64,
    pub created_at: Timestamp,
    pub modified_at: Timestamp, // TODO do I want to differentiate modify and change?
    pub block_data: BlockListHead,
    pub name: NodePointer<BlockString>,
}

/// A pointer of type `T` into a [crate::interface::BlockDevice].
#[derive(Debug, PartialEq, Eq)]
#[repr(transparent)]
// TODO rename
pub struct NodePointer<T> {
    pub lba: LBA,
    _block_type: PhantomData<T>,
}
const_assert!(size_of::<NodePointer<u8>>() == size_of::<LBA>());

impl<T> Clone for NodePointer<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for NodePointer<T> {}

impl<T> NodePointer<T> {
    pub fn new(lba: LBA) -> Self {
        Self {
            lba,
            _block_type: PhantomData,
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
    pub root: NodePointer<TreeNode>,
    /// Node pointer for the data required for the [crate::block_allocator::BlockAllocator]
    pub free_blocks: NodePointer<FreeBlockGroups>,
    /// A copy of the header, that should be kept in sync with the main header in the 0th block
    pub backup_header: NodePointer<MainHeader>,
    /// A name for the filesystem
    pub name: Option<NodePointer<BlockString>>,
    /// Transient data that describe store the current state of the filesystem.
    ///
    /// In theory this data should not be required on disk, but might be useful for recovery
    /// and error detection.
    pub transient: Option<MainTransientHeader>,
}
const_assert!(size_of::<MainHeader>() <= BLOCK_SIZE);

impl MainHeader {
    /// A magic string that must be part of the header
    pub const MAGIC: [u8; 8] = *b"WasabiFs";

    pub fn matches_backup(&self, backup: &Self) -> bool {
        assert!(self.transient.is_some());
        assert_ne!(self.backup_header.lba, MAIN_HEADER_BLOCK);

        let mut main_copy = self.clone();
        main_copy.backup_header = NodePointer::new(MAIN_HEADER_BLOCK);
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

pub(crate) const LEAVE_MAX_FILE_COUNT: usize = 6;
const_assert!(LEAVE_MAX_FILE_COUNT % 2 == 0);

pub(crate) const NODE_MAX_CHILD_COUNT: usize = 30;
const_assert!(NODE_MAX_CHILD_COUNT % 2 == 0);
// this simplifies rebalance
const_assert!(NODE_MAX_CHILD_COUNT / 2 >= 2);

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C, u8)]
pub enum TreeNode {
    Leave {
        parent: Option<NodePointer<TreeNode>>, // TODO do I need the parent pointer?
        files: StaticVec<FileNode, LEAVE_MAX_FILE_COUNT, u8>,
    },
    Node {
        /// The parent of this Node or `None` if this is the root node
        parent: Option<NodePointer<TreeNode>>, // TODO do I need the parent pointer?
        /// a list of [TreeNode] pointers and their maximum [FileId] value.
        ///
        /// `children[i].0 == children[i].1.follow().max`
        children: StaticVec<(NodePointer<TreeNode>, Option<FileId>), NODE_MAX_CHILD_COUNT, u8>,
    },
}
const_assert!(size_of::<TreeNode>() <= BLOCK_SIZE);

/// The number of free [BlockGroup]s that fit within a single [FreeBlockGroups]
pub(crate) const BLOCK_RANGES_COUNT_PER_BLOCK: usize = 31;

/// Used to keep track of unsused [BlockGroup]s.
///
/// This should be accessed through [crate::block_allocator::BlockAllocator].
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct FreeBlockGroups {
    /// Unused [BlockGroup]s
    pub free: StaticVec<BlockGroup, BLOCK_RANGES_COUNT_PER_BLOCK, u8>,
    /// A [NodePointer] to the next [FreeBlockGroups] of unused [BlockGroup]s.
    ///
    /// This might be `Some` even if [Self::free] is not full.
    /// The [crate::block_allocator::BlockAllocator] might uses empty [FreeBlockGroups] as reserved blocks.
    pub next: Option<NodePointer<FreeBlockGroups>>,
}
const_assert!(size_of::<FreeBlockGroups>() <= BLOCK_SIZE);
