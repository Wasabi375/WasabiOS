use core::{
    mem::MaybeUninit,
    ptr::NonNull,
    sync::atomic::{AtomicPtr, Ordering},
};

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc, vec::Vec};
use staticvec::StaticVec;

use crate::{
    fs_structs::{INode, INodeData, MainHeader, NodePointer, TreeNode},
    interface::BlockDevice,
};

/// An in memory view of the on device inode tree
///
/// Any modification of the inode tree is done on this data structure
/// and later saved to the device using a copy on write system, meaning
/// that the [MainHeader::root] will always point to a complete and valid
/// inode tree.
///
/// TODO: how do I ensure that old "used" blocks stay around until the main root is updated
///
/// # Data Structure
///
/// Both the on device tree([TreeNode]) and the in memory view are represented
/// as a [B+Tree] or to be more exact a [B-Tree] where the inodes are stored
/// in the leaves. The keys in internal nodes instead represent the largest key
/// of the corresponding subtree.
///
/// # Copy on Write -- TODO
///
/// # References
///
/// Also see [TreeNode] and [MainHeader::root]
///
/// [B-Tree]: https://en.wikipedia.org/wiki/B-tree#In_filesystems
/// [B+Tree]: https://en.wikipedia.org/wiki/B%2B_tree
pub struct MemTree {
    root: MemTreeLink,

    /// The maximum number of children each node can have
    ///
    /// This  is also refered to as the order of the B-Tree, as defined by Knuth.
    ///
    /// See: <https://en.wikipedia.org/wiki/B-tree#Differences_in_terminology>
    max_child_count: usize,

    dirty: bool,
}

impl !Clone for MemTree {}

impl MemTree {
    fn min_child_count(&self) -> usize {
        (self.max_child_count + 1) / 2
    }

    pub fn find<D, P>(
        &mut self,
        device: &D,
        inode: INode,
    ) -> Result<Option<INodeData>, D::BlockDeviceError>
    where
        D: BlockDevice,
    {
        todo!()
    }
}

enum MemTreeNode {
    Node {
        parent: Option<WeakMemTreeLink>,
        children: Vec<(INode, MemTreeLink)>,
        dirty: bool,
    },
    Leave {
        parent: WeakMemTreeLink,
        inodes: Vec<INodeData>,
        dirty: bool,
    },
}

impl MemTreeNode {
    fn is_full(&self) -> bool {
        match self {
            MemTreeNode::Node { children, .. } => children.len() == children.capacity(),
            MemTreeNode::Leave { inodes, .. } => inodes.len() == inodes.capacity(),
        }
    }

    fn dirty(&self) -> bool {
        match self {
            MemTreeNode::Node { dirty, .. } => *dirty,
            MemTreeNode::Leave { dirty, .. } => *dirty,
        }
    }

    fn find<D: BlockDevice>(
        &self,
        device: &D,
        tree: &mut MemTree,
        inode: INode,
    ) -> Result<Option<INodeData>, D::BlockDeviceError> {
        match self {
            MemTreeNode::Node { children, .. } => todo!(),
            MemTreeNode::Leave { inodes, .. } => todo!(),
        }
    }
}

impl From<TreeNode> for MemTreeNode {
    fn from(value: TreeNode) -> Self {
        todo!()
    }
}

/// A pointer used within a [MemTree].
///
/// The data may or may not be loaded into memory at any point.
/// [MemTreeLink::resolve] should be used to load the data from device
/// into memory, before accessing the inner data.
struct MemTreeLink {
    node: AtomicPtr<MemTreeNode>,
    /// The device pointer where the data is stored.
    ///
    /// The on device data will not reflect any changes done to the in memory
    /// copy. See [MemTree] for the Copy on Write description.
    device_ptr: NodePointer<TreeNode>,
}

/// This is required for the safety guarantees in [MemTreeLink]
impl !Clone for MemTreeLink {}

impl MemTreeLink {
    /// Ensures that the link data is loaded into memory.
    #[inline]
    fn resolve<D: BlockDevice>(&self, device: &D) -> Result<(), D::BlockDeviceError> {
        if !self.node.load(Ordering::Acquire).is_null() {
            return Ok(());
        }

        let dev_node = unsafe { device.read_pointer(self.device_ptr)? };
        let in_mem_raw = Box::into_raw(Box::new(dev_node.into()));

        match self.node.compare_exchange(
            core::ptr::null_mut(),
            in_mem_raw,
            Ordering::AcqRel,
            Ordering::Acquire, // NOTE: it should be fine to make this relaxed
        ) {
            Ok(_) => Ok(()),
            Err(_) => {
                // someone else was faster in resolving this node
                unsafe {
                    // Safety: this was created from a box and we failed to store it in the atomic
                    // ptr
                    Box::from_raw(in_mem_raw);
                }
                Ok(())
            }
        }
    }

    /// Drops the *in memory* representation of the Link
    ///
    /// This has no effect if the link is not resolved into memory.
    /// See [MemTreeLink::resolve]
    ///
    /// # Panic
    ///
    /// Panics if the in memory node is dirty. See [MemTreeNode] for more
    /// information about the dirty state.
    ///
    /// # Returns
    ///
    /// Return the in memory representation of the [MemTreeNode] or None
    /// if there was none.
    ///
    /// TODO: Is the fact that I return a Box an implementation detail I want to hide?
    ///
    /// # Safety
    ///
    /// The caller must gurantee that no other reference to the underlying data exists.
    /// References can be obtained using [Self::get] and [Self::get_mut]
    fn drop_in_mem(&mut self, tree: &mut MemTree) -> Option<Box<MemTreeNode>> {
        let ptr = self.node.swap(core::ptr::null_mut(), Ordering::AcqRel);

        if ptr.is_null() {
            None
        } else {
            let node = unsafe {
                // Safety: we have &mut self and removed any later references
                Box::from_raw(ptr)
            };
            Some(node)
        }
    }

    #[inline]
    fn get(&self) -> Option<&MemTreeNode> {
        let ptr = self.node.load(Ordering::Acquire);

        if ptr.is_null() {
            None
        } else {
            // Safety: &self access
            Some(unsafe { &*ptr })
        }
    }

    #[inline]
    fn get_mut(&mut self) -> Option<&mut MemTreeNode> {
        let ptr = self.node.load(Ordering::Acquire);

        if ptr.is_null() {
            None
        } else {
            // Safety: &mut self access
            Some(unsafe { &mut *ptr })
        }
    }
}

/// A weak link in the [MemTree]
///
/// This is used for backreferences and does not provide ownership or
/// any safe access.
struct WeakMemTreeLink(NonNull<MemTreeNode>);
