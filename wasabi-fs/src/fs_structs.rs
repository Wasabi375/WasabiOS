use core::{
    default,
    marker::PhantomData,
    mem::size_of,
    num::{NonZeroU16, NonZeroU64, NonZeroU8},
};

use static_assertions::const_assert;
use staticvec::{StaticString, StaticVec};
use uuid::Uuid;

use crate::{BlockGroup, BLOCK_SIZE, LBA};

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

const BLOCK_STRING_DATA_LENGTH: usize =
    512 - (size_of::<u32>() + size_of::<Option<NodePointer<BlockStringPart>>>());

/// A string stored over 1 or multiple blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct BlockString {
    pub length: u32,
    pub data: [u8; BLOCK_STRING_DATA_LENGTH],
    pub next: Option<NodePointer<BlockStringPart>>,
}
const_assert!(size_of::<BlockString>() == BLOCK_SIZE);

const BLOCK_STRING_PART_DATA_LENGTH: usize =
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
/// Nodes can be things like files, directories, etc. See [INodeType]
///
/// Unique means unique within this filesystem.
/// It is possible if not likely for INodes to be the same on different file systems
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct INode(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum INodeType {
    File,
    Directory,
}

/// A timestamp
///
/// Right now this is just a placeholder to reserve memory for future use.
/// It'll probably end up as unix epoch time in ms, but this might change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Timestamp(u64);

const I_NODE_MAX_NAME_LEN: usize = 40;
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct INodeData {
    pub inode: INode,
    pub parent: INode,
    pub typ: INodeType,
    _unused: [u8; 7],
    pub created_at: Timestamp,
    pub modified_at: Timestamp,
    pub block_data: BlockListHead,
    pub name: NodePointer<BlockString>,
}

/// A pointer of type `T` into a [crate::interface::BlockDevice].
#[derive(Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct NodePointer<T> {
    pub lba: LBA,
    _block_type: PhantomData<T>,
}
const_assert!(size_of::<NodePointer<u8>>() == size_of::<LBA>());

impl<T> Clone for NodePointer<T> {
    fn clone(&self) -> Self {
        Self::new(self.lba)
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
    pub free_blocks: NodePointer<FreeBlockGroups>,
    pub name: Option<NodePointer<BlockString>>,
    pub transient: Option<MainTransientHeader>,
}
const_assert!(size_of::<MainHeader>() <= BLOCK_SIZE);

impl MainHeader {
    /// A magic string that must be part of the header
    pub const MAGIC: [u8; 8] = *b"WasabiFs";
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct MainTransientHeader {
    pub mount_count: u8,
    pub writeable: bool,
    pub status: FsStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C, u8)]
pub enum TreeNode {
    Leave {
        parent: Option<NodePointer<TreeNode>>,
        nodes: StaticVec<INodeData, 6, u8>,
    },
    Node {
        parent: Option<NodePointer<TreeNode>>,
        children: StaticVec<(INode, NodePointer<TreeNode>), 30, u8>,
    },
}
const_assert!(size_of::<TreeNode>() <= BLOCK_SIZE);

/// The number of free [BlockGroup]s that fit within a single [FreeBlockGroups]
pub(crate) const BLOCK_RANGES_COUNT_PER_BLOCK: usize = 31;

/// Used to keep track of unsused [BlockGroup]s.
///
/// This should be accessed through [crate::mem_structs::BlockAllocator].
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct FreeBlockGroups {
    /// Unused [BlockGroup]s
    pub free: StaticVec<BlockGroup, BLOCK_RANGES_COUNT_PER_BLOCK, u64>,
    /// A [NodePointer] to the next [FreeBlockGroups] of unused [BlockGroup]s.
    ///
    /// This might be `Some` even if [Self::free] is not full.
    /// The [crate::mem_structs::BlockAllocator] might uses empty [FreeBlockGroups] as reserved blocks.
    pub next: Option<NodePointer<FreeBlockGroups>>,
}
const_assert!(size_of::<FreeBlockGroups>() <= BLOCK_SIZE);
