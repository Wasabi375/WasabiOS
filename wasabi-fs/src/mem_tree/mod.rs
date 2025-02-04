use core::{
    alloc::AllocError,
    assert_matches::assert_matches,
    cell::UnsafeCell,
    error,
    mem::{self, MaybeUninit},
    ops::Deref,
    ptr::{null_mut, NonNull},
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use alloc::{
    boxed::{self, Box},
    collections::BTreeMap,
    sync::Arc,
    vec::Vec,
};
use log::{debug, trace};
use shared::sync::{lockcell::UnsafeTicketLock, InterruptState};
use staticvec::{staticvec, StaticVec};
use thiserror::Error;

use crate::{
    fs_structs::{
        FileId, FileNode, MainHeader, NodePointer, TreeNode, LEAVE_MAX_FILE_COUNT,
        NODE_MAX_CHILD_COUNT,
    },
    interface::BlockDevice,
};

use self::iter::MemTreeNodeIter;

pub mod iter;

#[derive(Error, Debug, PartialEq, Eq, Clone)]
#[allow(missing_docs)]
pub enum MemTreeError<D: BlockDevice> {
    #[error("Out of Memory")]
    Oom(#[from] AllocError), // TODO ensure I don't destroy the locks
    #[error("Block Device Failure: {0}")]
    BlockDevice(D::BlockDeviceError), // TODO ensure I don't destroy the locks
    #[error("FileNode with id {0} already exists")]
    FileNodeExists(FileId),
}

impl<D: BlockDevice> MemTreeError<D> {
    pub fn from(value: D::BlockDeviceError) -> Self {
        Self::BlockDevice(value)
    }
}

/// An in memory view of the on device file tree
///
/// Any modification of the file tree is done on this data structure
/// and later saved to the device using a copy on write system, meaning
/// that the [MainHeader::root] will always point to a complete and valid
/// file tree.
///
/// TODO: how do I ensure that old "used" blocks stay around until the main root is updated
///
/// # Data Structure
///
/// Both the on device tree([TreeNode]) and the in memory view are represented
/// as a [B+Tree] or to be more exact a [B-Tree] where the files are stored
/// in the leaves. The keys in internal nodes instead represent the largest key
/// of the corresponding subtree.
///
/// ## Locking
///
/// TODO implement and describe fine grained B-Tree locking:
///
/// * https://runshenzhu.github.io/618-final/: Approach #2: Fine Grained Locking
/// * The Ubiquitous B-Tree (Douglas Comer): 4. B-Trees in a Multiuser Environmen
///
/// # Copy on Write
///
/// TODO describe/figure out how/when updates are stored to device, etc
///
/// # References
///
/// Also see [TreeNode] and [MainHeader::root]
///
/// [B-Tree]: https://en.wikipedia.org/wiki/B-tree#In_filesystems
/// [B+Tree]: https://en.wikipedia.org/wiki/B%2B_tree
pub struct MemTree<I> {
    /// must only be accesed while [Self::root_lock] is held
    root: UnsafeCell<MemTreeLink<I>>,

    /// lock for [Self::root]
    root_lock: UnsafeTicketLock<I>,

    dirty: AtomicBool,
}
// TODO impl Drop for MemTree

impl<I> !Clone for MemTree<I> {}

/// Access mode for finding a leave
#[derive(Debug, Clone, Copy)]
enum AccessMode {
    /// only lock the current node
    Readonly,
    /// lock all nodes and set the dirty_children flag
    Update,
}

impl<I: InterruptState> MemTree<I> {
    /// Gets the root node
    ///
    /// # Safety
    ///
    /// The caller muset ensure the root_lock is held
    unsafe fn get_root(&self) -> &MemTreeLink<I> {
        unsafe { &*self.root.get() }
    }

    /// Gets the root node
    ///
    /// # Safety
    ///
    /// The caller muset ensure the root_lock is held
    unsafe fn get_root_mut(&self) -> &mut MemTreeLink<I> {
        unsafe { &mut *self.root.get() }
    }

    /// Return the leave that should contain `id` if present.
    ///
    /// The nodes are locked based on [LockStrategy]
    fn find_leave<D: BlockDevice>(
        &self,
        device: &D,
        id: FileId,
        access_mode: AccessMode,
    ) -> Result<&mut MemTreeNode<I>, D::BlockDeviceError> {
        self.root_lock.lock();

        let mut current_node = unsafe {
            // Safety: we hold the root lock
            self.get_root_mut().resolve(device)?.as_mut()
        }
        .expect("we just resolved the node");

        current_node.get_lock().lock();

        if matches!(access_mode, AccessMode::Readonly) {
            unsafe {
                // Safety: we just locked this
                self.root_lock.unlock();
            }
        }

        'outer: loop {
            match current_node {
                MemTreeNode::Node {
                    children,
                    lock,
                    dirty_children,
                    ..
                } => {
                    if matches!(access_mode, AccessMode::Update) {
                        *dirty_children = true;
                    }

                    for (child, max_id) in children.iter_mut() {
                        if id <= max_id.unwrap_or(FileId::MAX) {
                            child.resolve(device)?;
                            current_node = unsafe {
                                // Safety: we just locked this
                                child.as_mut().expect("we just resolved the node")
                            };
                            current_node.get_lock().lock();
                            if matches!(access_mode, AccessMode::Readonly) {
                                unsafe {
                                    // Safety: we locked this in the previous iteration (child_lock)
                                    lock.unlock();
                                }
                            }
                            continue 'outer;
                        }
                    }
                    unreachable!("max_id should always be None for the last entry")
                }
                leave @ MemTreeNode::Leave { .. } => return Ok(leave),
            }
        }
    }

    /// Unlocks node and all above nodes
    ///
    /// # Safety
    ///
    /// node, all above nodes and the root_lock must be locked
    unsafe fn unlock_upwards(&self, node: &MemTreeNode<I>) {
        let mut current = node;

        loop {
            let parent = current.get_parent();

            unsafe {
                // Safety: current is locked
                current.get_lock().unlock();
            }

            if let Some(parent) = parent {
                current = unsafe {
                    // Safety: we have the parent lock
                    parent.as_ref()
                }
            } else {
                unsafe {
                    // Safety: root is locked
                    self.root_lock.unlock();
                }
                break;
            }
        }
    }

    pub fn find<D: BlockDevice>(
        &self,
        device: &D,
        id: FileId,
    ) -> Result<Option<FileNode>, D::BlockDeviceError> {
        let leave: &_ = self.find_leave(device, id, AccessMode::Readonly)?;
        let MemTreeNode::Leave { files, lock, .. } = leave else {
            panic!("Find leave should always return a leave node");
        };

        let result = files.iter().find(|f| f.id == id).cloned();
        unsafe {
            // Safety: we locked this in the previous iteration
            // (child_lock in Node/root case)
            lock.unlock();
        }
        return Ok(result);
    }

    pub fn insert<D: BlockDevice>(
        &self,
        device: &D,
        file: FileNode,
        create_only: bool, // TODO do I want to create to functions here? Insert/update
    ) -> Result<(), MemTreeError<D>> {
        let leave = self
            .find_leave(device, file.id, AccessMode::Update)
            .map_err(MemTreeError::from)?;

        let MemTreeNode::Leave {
            parent,
            files,
            dirty,
            lock,
        } = leave
        else {
            panic!("Find leave should always return a leave node");
        };
        assert!(!lock.is_unlocked());

        if let Some(override_pos) = files.iter().position(|f| f.id == file.id) {
            if create_only {
                unsafe {
                    // Safety: leave and all above nodes are locked
                    self.unlock_upwards(leave)
                }

                return Err(MemTreeError::FileNodeExists(file.id));
            }

            *dirty = true;
            files[override_pos] = file;

            unsafe {
                // Safety: leave and all above nodes are locked
                self.unlock_upwards(leave)
            }

            return Ok(());
        }

        *dirty = true;

        let insert_pos = files
            .iter()
            .position(|f| f.id > file.id)
            .unwrap_or(files.len());

        if files.is_not_full() {
            files.insert(insert_pos, file);

            unsafe {
                // Safety: leave and all above nodes are locked
                self.unlock_upwards(leave)
            }

            return Ok(());
        }

        let half_capacity = files.capacity() / 2;
        let mut right_node_files = files.drain(half_capacity..);
        let left_node_files = files;
        assert_eq!(left_node_files.len(), right_node_files.len());

        if insert_pos <= half_capacity {
            left_node_files.insert(insert_pos, file);
        } else {
            right_node_files.insert(insert_pos - half_capacity, file);
        }

        unsafe {
            // Safety: the tree is locked, we no longer need to access this leave
            // so we can unlock it and continue with updating the nodes above us.
            lock.unlock();
        }

        let split_id = left_node_files
            .last()
            .expect("This should never be empty")
            .id;
        let new_node_max = right_node_files
            .last()
            .expect("This should never be empty")
            .id;

        let new_node = MemTreeNode::Leave {
            parent: parent.clone(),
            files: right_node_files,
            dirty: true,
            lock: UnsafeTicketLock::new(),
        };

        match parent {
            Some(parent) => {
                let parent = unsafe {
                    // Safety: we still hold the locks for all nodes above the leave
                    parent.as_mut()
                };
                unsafe {
                    // Safety: we still hold the locks for all nodes above the leave
                    self.insert_split(parent, leave, split_id, new_node, new_node_max)
                }
            }
            None => unsafe {
                // Safety: we still hold the locks for all nodes above the leave
                self.insert_split_at_root(leave, split_id, new_node)
            },
        }
    }

    /// Insert the `new_node` that was created by a split
    ///
    /// TODO document args
    ///
    /// # Safety
    ///
    /// root_lock must be held
    unsafe fn insert_split<D: BlockDevice>(
        &self,
        parent: &mut MemTreeNode<I>,
        old_node: &mut MemTreeNode<I>,
        old_node_new_max: FileId,
        new_node: MemTreeNode<I>,
        new_node_max: FileId,
    ) -> Result<(), MemTreeError<D>> {
        let MemTreeNode::Node {
            parent: next_parent,
            children,
            dirty,
            dirty_children,
            lock,
        } = parent
        else {
            panic!("parent must always be a node");
        };
        trace!("insert_split");

        *dirty = true;

        let new_node_link = MemTreeLink {
            node: Some(Box::try_new(new_node)?),
            device_ptr: None,
        };

        let old_position = children
            .iter()
            .position(|(child, _max)| {
                child.node.as_ref().map(|n| n.as_ref() as *const _) == Some(old_node as *const _)
            })
            .expect("children should always contain the old node");

        let insert_position = old_position + 1;
        assert!(
            insert_position <= children.len(),
            "Insert must either move existing children to the right, or be added as the last child"
        );

        if children.is_not_full() {
            children[old_position].1 = Some(old_node_new_max);

            let new_node_max = Some(new_node_max).take_if(|_| insert_position != children.len());
            children.insert(insert_position, (new_node_link, new_node_max));

            unsafe {
                // Safety: parent and all above nodes are locked
                // old_node is not locked as it was unlocked in the previous recursion
                // (parent/leave)
                self.unlock_upwards(parent)
            }

            return Ok(());
        }

        let half_capacity = children.capacity() / 2;
        let mut right_node_children = children.drain(half_capacity..);
        assert_eq!(children.len(), right_node_children.len());

        if insert_position <= half_capacity {
            let new_node_max = Some(new_node_max).take_if(|_| insert_position != children.len());
            children.insert(insert_position, (new_node_link, new_node_max));
        } else {
            let insert_position = insert_position - half_capacity;
            let new_node_max =
                Some(new_node_max).take_if(|_| insert_position != right_node_children.len());
            right_node_children.insert(insert_position, (new_node_link, new_node_max));
        }

        unsafe {
            // Safety: the tree is locked from this node up, we no longer need to access this node
            // so we can unlock it and continue with updating the nodes above us.
            lock.unlock();
        }

        let split_id = children
            .last()
            .expect("This should never be empty")
            .1
            .expect("This node was in the middle of the children array and therefor should have a max value");

        let new_node = MemTreeNode::Node {
            parent: next_parent.clone(),
            children: right_node_children,
            dirty: true,
            dirty_children: true,
            lock: UnsafeTicketLock::new(),
        };

        match next_parent {
            Some(parent) => {
                let parent = unsafe {
                    // Safety: we still hold the locks for all nodes above the leave
                    parent.as_mut()
                };
                unsafe {
                    // Safety: we still hold the locks for all nodes above the leave
                    self.insert_split(parent, old_node, split_id, new_node, new_node_max)
                }
            }
            None => unsafe {
                // Safety: we still hold the locks for all nodes above the leave
                self.insert_split_at_root(old_node, split_id, new_node)
            },
        }
    }

    /// Insert the `new_node` that was created by a split at the root level
    ///
    /// `root` should be the tree root and `new_node` should be the new right
    /// subtree after the slpit.
    ///
    /// # Safety
    ///
    /// root_lock must be held
    unsafe fn insert_split_at_root<D: BlockDevice>(
        &self,
        old_root: &mut MemTreeNode<I>,
        split_id: FileId,
        new_node: MemTreeNode<I>,
    ) -> Result<(), MemTreeError<D>> {
        let old_root_link = unsafe {
            // Safety: we still hold the root_lock
            self.get_root_mut()
        };
        let tree_root = unsafe {
            // Safety: we still hold the root_lock
            old_root_link
                .as_ref()
                .expect("Tree root should be resolved at this point")
        };
        assert_eq!(
            tree_root as *const _, old_root as *const _,
            "Expected `root` to match the tree root"
        );

        trace!("insert_split_at_root");

        // fill root with a temp dummy value
        let left_link = core::mem::replace(
            old_root_link,
            MemTreeLink {
                node: None,
                device_ptr: None,
            },
        );

        let mut right_node = Box::try_new(new_node)?;
        let right_link = MemTreeLink {
            node: Some(right_node),
            device_ptr: None,
        };

        let children = [(left_link, Some(split_id)), (right_link, None)]
            .into_iter()
            .collect();

        let mut new_root = Box::try_new(MemTreeNode::Node {
            parent: None,
            children,
            dirty: true,
            dirty_children: true,
            lock: UnsafeTicketLock::new(),
        })?;

        let new_root_ptr = Some(NonNull::from(new_root.as_ref()));
        let MemTreeNode::Node { children, .. } = new_root.as_mut() else {
            unreachable!()
        };
        for child in children {
            let child = unsafe {
                // Safety: we have unique access so we can ignore the lock
                child.0.as_mut().expect("We just filled this")
            };
            *child.get_parent_mut() = new_root_ptr;
        }
        let new_root = MemTreeLink {
            node: Some(new_root),
            device_ptr: None,
        };
        unsafe {
            // Safety: we hold the root_lock
            *self.root.get() = new_root;
        }

        unsafe {
            // Safety: we hold the root_lock
            self.root_lock.unlock();
        }

        Ok(())
    }

    pub fn assert_valid(&self) {
        self.root_lock.lock();

        // Safety: we have the root lock
        let root = unsafe { self.get_root().as_ref().unwrap() };

        root.assert_valid(None);

        unsafe {
            // Safety: locked above
            self.root_lock.unlock();
        }
    }
}

#[cfg(feature = "test")]
impl<I: InterruptState> MemTree<I> {
    /// Create an iterator over all nodes.
    ///
    /// This iterator returns all resolved [MemTreeNode]s from left to right,
    /// one level at a time, e.g. root, first node in root, second node in root, leaves
    fn iter_nodes(&mut self) -> MemTreeNodeIter<'_, I> {
        MemTreeNodeIter::new(self)
    }

    fn assert_valid_full(&mut self) {
        // check all leaves are on the same level
        let mut found_leave = false;
        for node in self.iter_nodes() {
            match node {
                MemTreeNode::Node { lock, .. } => {
                    assert!(lock.is_unlocked());
                    assert!(!found_leave);
                }
                MemTreeNode::Leave { lock, .. } => {
                    assert!(lock.is_unlocked());
                    found_leave = true
                }
            }
        }
        assert!(found_leave);

        self.assert_valid();
    }
}

pub(crate) enum MemTreeNode<I> {
    Node {
        parent: Option<NonNull<MemTreeNode<I>>>,
        /// child pointers to [MemTreeNode] and the largest file id of the subtree.
        ///
        /// max `file_id` is `None` for the last entry
        children: StaticVec<(MemTreeLink<I>, Option<FileId>), NODE_MAX_CHILD_COUNT>,
        /// set if it is modified directly.
        ///
        /// If this is `false`, it does not mean that all `children` `dirty` flags are also set
        /// to `false`.
        /// The only way to find all dirty nodes is to iterate the entire resolved tree.
        dirty: bool,
        /// set if any children or their children are dirty
        dirty_children: bool,
        /// lock for updating this node
        lock: UnsafeTicketLock<I>,
    },
    Leave {
        parent: Option<NonNull<MemTreeNode<I>>>,

        /// files sorted based on their id
        ///
        /// PERF should I create some indirection here? Moving files within this list to keep this
        /// sorted might be slow
        files: StaticVec<FileNode, LEAVE_MAX_FILE_COUNT>,
        /// set if any file is modfied.
        dirty: bool,
        /// lock for updating this node
        lock: UnsafeTicketLock<I>,
    },
}

impl<I: InterruptState> MemTreeNode<I> {
    fn new_from(device_node: TreeNode) -> Self {
        todo!()
        //        match device_node {
        //            TreeNode::Leave { parent: _, files } => MemTreeNode::Leave {
        //                files: files.to_vec(),
        //                dirty: AtomicBool::new(false),
        //            },
        //            TreeNode::Node {
        //                parent: _,
        //                mut children,
        //            } => MemTreeNode::Node {
        //                children: children
        //                    .drain_iter(..)
        //                    .map(|(id, node_ptr)| {
        //                        (
        //                            id,
        //                            MemTreeLink {
        //                                node: Atomic::null(),
        //                                device_ptr: node_ptr,
        //                            },
        //                        )
        //                    })
        //                    .collect(),
        //                dirty: AtomicBool::new(false),
        //            },
        //        }
    }

    fn dirty(&self) -> bool {
        match self {
            MemTreeNode::Node { dirty, .. } => *dirty,
            MemTreeNode::Leave { dirty, .. } => *dirty,
        }
    }

    fn get_lock(&self) -> &UnsafeTicketLock<I> {
        match self {
            MemTreeNode::Node { lock, .. } => lock,
            MemTreeNode::Leave { lock, .. } => lock,
        }
    }

    fn len(&self) -> usize {
        match self {
            MemTreeNode::Node { children, .. } => children.len(),
            MemTreeNode::Leave { files, .. } => files.len(),
        }
    }

    fn cap(&self) -> usize {
        match self {
            MemTreeNode::Node { children, .. } => children.capacity(),
            MemTreeNode::Leave { files, .. } => files.capacity(),
        }
    }

    fn get_parent(&self) -> Option<NonNull<MemTreeNode<I>>> {
        match self {
            MemTreeNode::Node { parent, .. } => *parent,
            MemTreeNode::Leave { parent, .. } => *parent,
        }
    }

    fn get_parent_mut(&mut self) -> &mut Option<NonNull<MemTreeNode<I>>> {
        match self {
            MemTreeNode::Node { parent, .. } => parent,
            MemTreeNode::Leave { parent, .. } => parent,
        }
    }
}

#[cfg(feature = "test")]
impl<I: InterruptState> MemTreeNode<I> {
    fn assert_valid(&self, parent: Option<&MemTreeNode<I>>) {
        let lock = self.get_lock();
        lock.lock();

        assert_eq!(
            parent.map(|p| p as *const _),
            self.get_parent()
                .map(|ptr| unsafe { ptr.as_ptr() as *const _ }),
            "Parent does not match"
        );

        if parent.is_some() {
            assert!(
                self.len() >= self.cap() / 2,
                "Min length guranteed by B-Tree for non root nodes"
            );
        }

        match self {
            MemTreeNode::Node { children, .. } => {
                for window in children.windows(2) {
                    let first = window[0].1;
                    let second = window[1].1;

                    match (first, second) {
                        (Some(first), Some(second)) => assert!(first < second),
                        (Some(_), None) => assert!(true),
                        (None, Some(_)) | (None, None) => {
                            assert!(false, "Only the last child should have a max of None")
                        }
                    }
                }
                assert!(children.last().unwrap().1.is_none());

                children
                    .iter()
                    .map(|child| &child.0)
                    .filter_map(|link| unsafe {
                        // we hold the current nodes lock
                        link.as_ref()
                    })
                    .for_each(|child| child.assert_valid(Some(self)));
            }
            MemTreeNode::Leave { files, .. } => {
                assert!(files.iter().is_sorted_by_key(|f| f.id));
            }
        }

        unsafe {
            // Safety: locked above
            lock.unlock();
        }
    }
}

/// A pointer used within a [MemTree].
///
/// The data may or may not be loaded into memory at any point.
/// [MemTreeLink::resolve] should be used to load the data from device
/// into memory, before accessing the inner data.
pub(crate) struct MemTreeLink<I> {
    node: Option<Box<MemTreeNode<I>>>,
    /// The device pointer where the data is stored.
    ///
    /// The on device data will not reflect any changes done to the in memory
    /// copy. See [MemTree] for the Copy on Write description.
    ///
    /// This is `None` if the node was just created and is not yet stored on the device
    device_ptr: Option<NodePointer<TreeNode>>,
}

/// This is required for the safety guarantees in [MemTreeLink]
///
/// This is not strictly true. Access to the MemTreeNode is guarded by locks,
/// but this is still usefull, as it prevents us from accidentally keeping
/// pointers around that we no longer "know" about.
impl<I> !Clone for MemTreeLink<I> {}

impl<I: InterruptState> MemTreeLink<I> {
    /// Ensures that the link data is loaded into memory.
    #[inline]
    fn resolve<D: BlockDevice>(&mut self, device: &D) -> Result<&mut Self, D::BlockDeviceError> {
        if self.node.is_some() {
            return Ok(self);
        }

        let Some(device_ptr) = self.device_ptr else {
            panic!("A node that is not stored in memory, should always be stored on device");
        };

        let device_node = unsafe {
            // Safety: device_ptr should be a valid pointer on the device
            device.read_pointer(device_ptr)?
        };

        self.node = Some(Box::new(MemTreeNode::new_from(device_node)));

        Ok(self)
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
    /// In this case the node is not dropped and "should" remain valid.
    ///
    /// # Returns
    ///
    /// Return the in memory representation of the [MemTreeNode] or None
    /// if there was none.
    ///
    /// # Safety
    ///
    /// The caller must gurantee that no other reference to the underlying data exists.
    /// References can be obtained using [Self::as_ref] and [Self::as_mut].
    /// The caller gurantees that the necessary locks are held
    unsafe fn drop_in_mem(&mut self, tree: &mut MemTree<I>) {
        todo!()
    }

    /// Get a reference to the [MemTreeNode]
    ///
    /// # Safety
    ///
    /// the caller must ensure he holds the necessray locks
    #[inline]
    unsafe fn as_ref(&self) -> Option<&MemTreeNode<I>> {
        unsafe { self.node.as_ref().map(|n| n.as_ref()) }
    }

    /// Get a mutable reference to the [MemTreeNode]
    ///
    /// # Safety
    ///
    /// the caller must ensure he holds the necessray locks
    #[inline]
    unsafe fn as_mut(&mut self) -> Option<&mut MemTreeNode<I>> {
        unsafe { self.node.as_mut().map(|n| n.as_mut()) }
    }
}

#[cfg(feature = "test")]
mod test_mem_only {
    use alloc::{boxed::Box, vec::Vec};
    use core::{
        ops::Deref,
        sync::atomic::{AtomicBool, AtomicPtr},
    };
    use log::{debug, trace};

    use shared::sync::lockcell::UnsafeTicketLock;
    use staticvec::StaticVec;
    use testing::{
        kernel_test, multiprocessor::TestInterruptState, t_assert, t_assert_eq, t_assert_matches,
        tfail, KernelTestError, TestUnwrapExt,
    };

    use crate::{
        fs_structs::{BlockListHead, FileId, FileNode, FileType, NodePointer, Perm, Timestamp},
        interface::test::TestBlockDevice,
        mem_tree::MemTreeError,
        BlockGroup, LBA,
    };

    use super::{MemTree, MemTreeLink, MemTreeNode};

    fn create_empty_tree() -> MemTree<TestInterruptState> {
        let root_node = MemTreeNode::Leave {
            parent: None,
            files: StaticVec::new(),
            dirty: false,
            lock: UnsafeTicketLock::new(),
        };

        let root = MemTreeLink {
            node: Some(Box::new(root_node)),
            device_ptr: None,
        };

        MemTree {
            root: root.into(),
            root_lock: UnsafeTicketLock::new(),
            dirty: AtomicBool::new(false),
        }
    }

    fn create_file_node(id: u64) -> FileNode {
        FileNode {
            id: FileId::try_new(id).unwrap(),
            parent: None,
            typ: FileType::File,
            permissions: [Perm::all(); 4],
            _unused: [0; 3],
            size: 0,
            created_at: Timestamp::zero(),
            modified_at: Timestamp::zero(),
            block_data: BlockListHead::Single(BlockGroup::new(
                LBA::new(0).unwrap(),
                LBA::new(0).unwrap(),
            )),
            name: NodePointer::new(LBA::new(0).unwrap()),
        }
    }

    #[kernel_test]
    fn test_insert_single() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(9), true)
            .tunwrap()?;

        t_assert!(tree.root_lock.is_unlocked());

        tree.assert_valid_full();

        let root = unsafe {
            // Safety: we have unique access right now and can ignore the lock
            tree.get_root().as_ref().unwrap()
        };

        if let MemTreeNode::Leave {
            parent,
            files,
            dirty,
            lock,
        } = root
        {
            t_assert!(parent.is_none());
            t_assert_eq!(&files.iter().map(|f| f.id.get()).collect::<Vec<_>>(), &[9]);
            t_assert!(dirty);
            t_assert!(lock.is_unlocked());
        } else {
            tfail!("Expected to find a leave node at tree root");
        }

        Ok(())
    }

    #[kernel_test]
    fn test_insert_no_split() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(9), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(8), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(17), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(49), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(78), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(30), true)
            .tunwrap()?;

        tree.assert_valid_full();

        let mut nodes = tree.iter_nodes();

        let root = nodes
            .next()
            .texpect("tree should always contain at least the root")?;

        if let MemTreeNode::Leave {
            parent,
            files,
            dirty,
            lock: _,
        } = root
        {
            t_assert!(parent.is_none());
            t_assert_eq!(
                &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
                &[8, 9, 17, 30, 49, 78]
            );
            t_assert!(dirty);
        } else {
            tfail!("Expected to find a leave node at tree root");
        }

        t_assert!(nodes.next().is_none());

        Ok(())
    }

    #[kernel_test]
    fn test_insert_split_root() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(67), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(15), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(89), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(11), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(97), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(52), true)
            .tunwrap()?;

        tree.assert_valid_full();

        tree.insert(&TestBlockDevice, create_file_node(62), true)
            .tunwrap()?;

        tree.assert_valid_full();

        let mut nodes = tree.iter_nodes();

        let root = nodes
            .next()
            .texpect("tree should always contain at least the root")?;

        let expected_left = &[11, 15, 52, 62];
        let expected_right = &[67, 89, 97];
        let expected_max = 62;

        let MemTreeNode::Node {
            children: root_children,
            ..
        } = root
        else {
            tfail!("Expected Node at root");
        };
        t_assert_eq!(
            &root_children
                .iter()
                .map(|c| c.1.map(FileId::get))
                .collect::<Vec<_>>(),
            &[Some(expected_max), None]
        );

        let left_leave = nodes.next().texpect("Failed to find left leave")?;
        let MemTreeNode::Leave {
            files: left_files, ..
        } = left_leave
        else {
            tfail!("Expected left leave");
        };

        t_assert_eq!(
            &left_files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
            expected_left
        );

        let right_leave = nodes.next().texpect("Failed to find right leave")?;
        let MemTreeNode::Leave {
            files: right_files, ..
        } = right_leave
        else {
            tfail!("Expected right leave");
        };

        t_assert_eq!(
            &right_files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
            expected_right
        );

        t_assert!(nodes.next().is_none());

        Ok(())
    }

    #[kernel_test]
    fn test_insert_split_node_insert_in_root() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(67), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(15), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(89), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(11), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(97), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(52), true)
            .tunwrap()?;

        // right befor root split
        tree.assert_valid_full();

        tree.insert(&TestBlockDevice, create_file_node(62), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(12), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(24), true)
            .tunwrap()?;

        // left node is full
        tree.assert_valid_full();

        tree.insert(&TestBlockDevice, create_file_node(5), true)
            .tunwrap()?;

        tree.assert_valid_full();

        Ok(())
    }

    #[kernel_test]
    fn test_insert_100_files() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        let file_ids: &[u64] = &[
            743, 152, 983, 284, 621, 847, 519, 366, 215, 790, 488, 951, 104, 325, 876, 267, 638,
            710, 499, 392, 827, 603, 140, 981, 236, 745, 812, 564, 952, 688, 427, 205, 999, 341,
            756, 674, 438, 810, 295, 158, 623, 892, 432, 273, 509, 611, 733, 856, 921, 684, 312,
            497, 134, 789, 268, 905, 740, 157, 372, 698, 829, 542, 624, 782, 451, 628, 195, 923,
            304, 574, 831, 269, 911, 156, 784, 393, 530, 629, 678, 851, 415, 27, 949, 673, 528,
            379, 604, 798, 15, 972, 748, 357, 600, 241, 19, 84, 43, 895, 305, 820,
        ];

        for id in file_ids {
            tree.insert(&TestBlockDevice, create_file_node(*id), true)
                .tunwrap()?;
            tree.assert_valid_full();
        }

        let file_count_in_tree: usize = tree
            .iter_nodes()
            .filter_map(|node| {
                if let MemTreeNode::Leave { files, .. } = node {
                    Some(files.len())
                } else {
                    None
                }
            })
            .sum();

        t_assert_eq!(file_count_in_tree, file_ids.len());

        Ok(())
    }

    #[kernel_test]
    fn test_insert_duplicate() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(12), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(12), false)
            .tunwrap()?;

        tree.assert_valid_full();

        let root = unsafe {
            // Safety: we have unique access right now and can ignore the lock
            tree.get_root().as_ref().unwrap()
        };

        if let MemTreeNode::Leave { files, .. } = root {
            t_assert_eq!(&files.iter().map(|f| f.id.get()).collect::<Vec<_>>(), &[12]);
        } else {
            tfail!("Expected to find a leave node at tree root");
        }

        Ok(())
    }

    #[kernel_test]
    fn test_insert_duplicate_no_override() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(12), true)
            .tunwrap()?;
        let err = tree.insert(&TestBlockDevice, create_file_node(12), true);

        t_assert_matches!(err, Err(MemTreeError::FileNodeExists(_)));

        tree.assert_valid_full();

        let root = unsafe {
            // Safety: we have unique access right now and can ignore the lock
            tree.get_root().as_ref().unwrap()
        };

        if let MemTreeNode::Leave { files, .. } = root {
            t_assert_eq!(&files.iter().map(|f| f.id.get()).collect::<Vec<_>>(), &[12]);
        } else {
            tfail!("Expected to find a leave node at tree root");
        }

        Ok(())
    }
}
