use core::{
    alloc::AllocError,
    assert_matches::assert_matches,
    cell::{RefCell, UnsafeCell},
    default, error,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    ops::Deref,
    ptr::{self, null_mut, NonNull},
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use alloc::{
    boxed::{self, Box},
    sync::Arc,
    vec::Vec,
};
use log::{debug, trace};
use shared::{
    sync::{lockcell::UnsafeTicketLock, InterruptState},
    todo_error, todo_warn,
};
use staticvec::{staticvec, StaticVec};
use testing_derive::multitest;
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
    #[error("FileNode with id {0} does not exist")]
    FileDoesNotExist(FileId),
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

impl<I> !Clone for MemTree<I> {}

/// Access mode for finding a leave
#[derive(Debug, Clone, Copy)]
#[deprecated]
enum AccessMode {
    /// only lock the current node
    Readonly,
    /// lock all nodes and set the dirty_children flag
    Update,
}

use node_guard::NodeGuard;
mod node_guard {
    use core::{marker::PhantomData, ptr::NonNull};

    use shared::sync::InterruptState;

    use super::MemTreeNode;

    pub trait Borrow {
        fn is_mut() -> bool;
        fn is_ref() -> bool {
            !Self::is_mut()
        }
    }
    pub trait BorrowMut: Borrow {}

    pub struct Mut {}
    pub struct Immut {}

    impl Borrow for Immut {
        fn is_mut() -> bool {
            false
        }
    }

    impl Borrow for Mut {
        fn is_mut() -> bool {
            true
        }
    }
    impl BorrowMut for Mut {}

    /// A reference to a MemTreeNode.
    ///
    /// This struct gurantees that the reference is valid and the necessary locks
    /// are held.
    pub struct NodeGuard<'a, I: InterruptState, BorrowType: Borrow> {
        node: NonNull<MemTreeNode<I>>,
        drop_check: bool,
        _lifetime: PhantomData<&'a ()>,
        _borrow: PhantomData<BorrowType>,
    }

    impl<I, B> !Clone for NodeGuard<'_, I, B> {}

    impl<I: InterruptState, B: Borrow> Drop for NodeGuard<'_, I, B> {
        fn drop(&mut self) {
            if self.drop_check {
                if B::is_mut() {
                    panic!(
                        "Mut Node guard must not be dropped. Call awaken or into_parent instead"
                    );
                } else {
                    unsafe {
                        // Safety: we hold the node lock, otherwise the initial reference is invalid.
                        self.borrow().get_lock().unlock();
                    }
                }
            }
        }
    }

    impl<'a, I: InterruptState, B: Borrow> NodeGuard<'a, I, B> {
        pub fn new(node: &'a mut MemTreeNode<I>) -> Self {
            Self {
                node: node.into(),
                drop_check: true,
                _lifetime: PhantomData,
                _borrow: PhantomData,
            }
        }

        pub fn awaken_ref(mut self) -> &'a MemTreeNode<I> {
            self.drop_check = false;
            // Safety: NodeGuard gurantees we have at least shared access
            unsafe { self.node.as_ref() }
        }

        pub fn borrow(&self) -> &MemTreeNode<I> {
            // Safety: NodeGuard gurantees we have at least shared access
            unsafe { self.node.as_ref() }
        }

        pub fn as_ptr(&self) -> NonNull<MemTreeNode<I>> {
            self.node
        }

        pub fn unlock(mut self) -> NonNull<MemTreeNode<I>> {
            self.drop_check = false;
            let node = self.borrow();
            unsafe {
                // Safety: we hold the node lock, otherwise the initial reference is invalid.
                node.get_lock().unlock();
            }

            self.as_ptr()
        }
    }

    impl<'a, I: InterruptState, B: BorrowMut> NodeGuard<'a, I, B> {
        pub fn awaken_mut(mut self) -> &'a mut MemTreeNode<I> {
            self.drop_check = false;
            // Safety: NodeGuard is BorrowMut, therefor we have unique access
            unsafe { self.node.as_mut() }
        }

        /// Get shared access to the parent node
        ///
        /// This still requires the current guard to have [AccessMode::Update] access,
        /// because this gurantees that the parent is still locked
        pub fn parent_ref<'b>(&'b self) -> Option<NodeGuard<'b, I, Immut>>
        where
            'a: 'b,
        {
            let node = self.borrow();

            let Some(parent_ptr) = node.get_parent() else {
                return None;
            };

            Some(NodeGuard {
                node: parent_ptr,
                drop_check: true,
                _lifetime: PhantomData,
                _borrow: PhantomData,
            })
        }

        /// Convert this guard into a guard of the parent
        ///
        /// This will unlock the current node, and also return a ptr to it
        pub fn into_parent(
            mut self,
        ) -> Result<(NodeGuard<'a, I, Mut>, NonNull<MemTreeNode<I>>), Self> {
            let node = self.borrow();

            let Some(parent_ptr) = node.get_parent() else {
                return Err(self);
            };
            unsafe {
                // Safety: we hold the node lock, otherwise the initial reference is invalid.
                node.get_lock().unlock();
            }
            self.drop_check = false;

            Ok((
                NodeGuard {
                    node: parent_ptr,
                    drop_check: true,
                    _lifetime: PhantomData,
                    _borrow: PhantomData,
                },
                self.node,
            ))
        }

        pub fn borrow_mut(&mut self) -> &mut MemTreeNode<I> {
            // Safety: NodeGuard is BorrowMut, therefor we have unique access
            unsafe { self.node.as_mut() }
        }
    }
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
    fn find_leave<D: BlockDevice, BorrowType: node_guard::Borrow>(
        &self,
        device: &D,
        id: FileId,
    ) -> Result<NodeGuard<'_, I, BorrowType>, D::BlockDeviceError> {
        self.root_lock.lock();

        let mut current_node = unsafe {
            // Safety: we hold the root lock
            self.get_root_mut().resolve(device)?.as_mut()
        }
        .expect("we just resolved the node");

        current_node.get_lock().lock();

        if BorrowType::is_ref() {
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
                    if BorrowType::is_mut() {
                        *dirty_children = true;
                    }

                    for (child, max_id) in children.iter_mut() {
                        if id <= max_id.unwrap_or(FileId::MAX) {
                            child.resolve(device)?;
                            current_node = unsafe {
                                // Safety: the link is held by current with was locked in the
                                // previous iteration
                                child.as_mut().expect("we just resolved the node")
                            };
                            current_node.get_lock().lock();
                            if BorrowType::is_ref() {
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
                leave @ MemTreeNode::Leave { .. } => return Ok(NodeGuard::new(leave)),
            }
        }
    }

    /// Finds the id of the file with the largest id.
    ///
    /// # Safety
    ///
    /// node must be locked and it's children must be unlocked
    unsafe fn find_largest_id<D: BlockDevice>(
        &self,
        node: &mut MemTreeNode<I>,
        device: &D,
    ) -> Result<Option<FileId>, D::BlockDeviceError> {
        assert!(!node.get_lock().is_unlocked());

        match node {
            MemTreeNode::Node { children, .. } => {
                let last_link = &mut children
                    .iter_mut()
                    .last()
                    .expect("B-Tree node can not be empty")
                    .0
                    .resolve(device)?;

                let last_child = unsafe {
                    // node is locked
                    last_link.as_mut().expect("just resolved")
                };
                last_child.get_lock().lock();
                let result = unsafe {
                    // Safety: just locked last_child
                    self.find_largest_id(last_child, device)
                };

                unsafe {
                    // Safety: locked above
                    last_child.get_lock().unlock();
                }

                result
            }
            MemTreeNode::Leave { files, .. } => Ok(files.iter().last().map(|f| f.id)),
        }
    }

    /// Unlocks node and all above nodes as well as the root_lock
    ///
    /// # Safety
    ///
    /// node, all above nodes and the root_lock must be locked
    unsafe fn unlock_upwards(&self, node: NodeGuard<'_, I, node_guard::Mut>) {
        let mut current = Ok(node);
        loop {
            match current {
                Ok(node) => {
                    current = node.into_parent().map(|(p, _)| p);
                }
                Err(root_guard) => {
                    root_guard.unlock();
                    unsafe {
                        self.root_lock.unlock();
                    }
                    return;
                }
            }
        }
    }

    pub fn find<D: BlockDevice>(
        &self,
        device: &D,
        id: FileId,
    ) -> Result<Option<Arc<FileNode>>, D::BlockDeviceError> {
        let leave = self.find_leave::<_, node_guard::Immut>(device, id)?;
        let MemTreeNode::Leave { files, lock, .. } = leave.borrow() else {
            panic!("Find leave should always return a leave node");
        };

        let result = files.iter().find(|f| f.id == id).cloned();

        return Ok(result);
    }

    pub fn insert<D: BlockDevice>(
        &self,
        device: &D,
        file: Arc<FileNode>,
        create_only: bool, // TODO do I want to create to functions here? Insert/update
    ) -> Result<(), MemTreeError<D>> {
        let mut leave = self
            .find_leave::<_, node_guard::Mut>(device, file.id)
            .map_err(MemTreeError::from)?;

        let MemTreeNode::Leave {
            parent,
            files,
            dirty,
            ..
        } = leave.borrow_mut()
        else {
            panic!("Find leave should always return a leave node");
        };

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
            log::trace!("simple insert");
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

        match leave.into_parent() {
            Ok((parent, leave_ptr)) => {
                unsafe {
                    // Safety: we still hold the locks for all nodes above the leave
                    self.insert_split(parent, leave_ptr, split_id, new_node, new_node_max)
                }
            }
            Err(root) => unsafe {
                // Safety: we still hold the locks for all nodes above the leave
                self.insert_split_at_root(root, split_id, new_node)
            },
        }
    }

    /// Insert the `new_node` that was created by a split in the parent
    ///
    /// TODO document args
    ///
    /// # Safety
    ///
    /// root_lock must be held
    unsafe fn insert_split<D: BlockDevice>(
        &self,
        mut current: NodeGuard<'_, I, node_guard::Mut>,
        old_node_ptr: NonNull<MemTreeNode<I>>,
        old_node_new_max: FileId,
        new_node: MemTreeNode<I>,
        new_node_max: FileId,
    ) -> Result<(), MemTreeError<D>> {
        let MemTreeNode::Node {
            parent: next_parent,
            children,
            dirty,
            dirty_children,
            ..
        } = current.borrow_mut()
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
                child.node.as_ref().map(|n| n.as_ref() as *const _)
                    == Some(old_node_ptr.as_ptr() as *const _)
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
                self.unlock_upwards(current)
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

        match current.into_parent() {
            Ok((parent, current_ptr)) => {
                unsafe {
                    // Safety: we still hold the locks for all nodes above the leave
                    self.insert_split(parent, current_ptr, split_id, new_node, new_node_max)
                }
            }
            Err(root) => unsafe {
                // Safety: we still hold the locks for all nodes above the leave
                self.insert_split_at_root(root, split_id, new_node)
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
        root: NodeGuard<'_, I, node_guard::Mut>,
        split_id: FileId,
        new_node: MemTreeNode<I>,
    ) -> Result<(), MemTreeError<D>> {
        trace!("insert_split_at_root");
        assert!(root.parent_ref().is_none());

        let (old_root_link_node, old_root_link_device) = {
            let old_root_link = unsafe {
                // Safety: we still hold the root_lock
                self.get_root_mut()
            };

            let root_node_ptr = root.unlock().as_ptr() as *const _;

            let old_root_link_node = old_root_link.node.take();
            assert!(old_root_link_node.is_some());

            let old_root_link_node_ptr = old_root_link_node
                .as_ref()
                .map(|node| Box::as_ptr(node))
                .expect("root node should be some");

            assert_eq!(old_root_link_node_ptr, root_node_ptr);

            (old_root_link_node, old_root_link.device_ptr.take())
        };

        let left_link = MemTreeLink {
            node: old_root_link_node,
            device_ptr: old_root_link_device,
        };

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

        let new_root_ptr = NonNull::from(new_root.as_mut());
        let MemTreeNode::Node { children, .. } = new_root.as_mut() else {
            unreachable!()
        };
        for child in children {
            let child = unsafe {
                // Safety: we have unique access so we can ignore the lock
                child.0.as_mut().expect("We just filled this")
            };
            *child.get_parent_mut() = Some(new_root_ptr);
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
            // Safety: we hold the root_lock. We unlocked every node
            self.root_lock.unlock();
        }

        Ok(())
    }

    /*
    pub fn delete<D: BlockDevice>(
        &self,
        device: &D,
        file_id: FileId,
    ) -> Result<Arc<FileNode>, MemTreeError<D>> {
        // NOTE: this will have to resolve on-device only nodes because we want to delete from the
        // device and not just the in memory FileNode. Therefor in order to keep the tree balanced
        // we have to find the file even if it is not in memory. Therefor it is ok to use
        // find_leave

        let leave = self
            .find_leave(device, file_id, AccessMode::Update)
            .map_err(MemTreeError::from)?;

        // TODO temp
        // leave.assert_valid_node_only();

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

        let Some(file_pos) = files.iter().position(|f| f.id == file_id) else {
            unsafe {
                // Safety: tree is locked from this node upwards
                self.unlock_upwards(leave);
            }
            return Err(MemTreeError::FileDoesNotExist(file_id));
        };

        let file_node = files.remove(file_pos);
        *dirty = true;

        if files.len() >= files.capacity() / 2 || parent.is_none() {
            unsafe {
                // Safety: tree is locked from this node upwards
                self.unlock_upwards(leave);
            }
            return Ok(file_node);
        }

        if parent.is_some() {
            unsafe {
                // Safety: tree is locked from this node upwards
                self.rebalance_leave(leave, device)
                    .map_err(MemTreeError::from)?;
            }
        }

        Ok(file_node)
    }

    /// Rebalances the tree using rotation and merges
    /// starting at a leave
    ///
    /// # Safety
    ///
    /// the tree must be locked from this node upwards
    unsafe fn rebalance_leave<D: BlockDevice>(
        &self,
        mut unbalanced_node: &mut MemTreeNode<I>,
        device: &D,
    ) -> Result<(), D::BlockDeviceError> {
        // TODO temp
        // unbalanced_node.assert_valid_node_only();

        let unbalanced_addr = unbalanced_node as *const _;
        let MemTreeNode::Leave { parent, files, .. } = unbalanced_node else {
            panic!("Expected leave and got node");
        };
        trace!("rebalance tree from leave");

        let mut parent = parent.expect("Expected non root node");
        let parent = unsafe {
            // Safety: the parent is locked and we have the only reference
            parent.as_mut()
        };
        let MemTreeNode::Node {
            children,
            parent: parent_parent,
            ..
        } = parent
        else {
            panic!("parent must always be a node");
        };

        let node_index = children
            .iter()
            .map(|c| c.0.node.as_ref().map(|node_box| Box::as_ptr(node_box)))
            .position(|c| c == Some(unbalanced_addr))
            .expect("parent should always contain it's children");

        if node_index >= 1 {
            let left = &mut children[node_index - 1].0;
            let left = unsafe {
                // Safety: parent is still locked
                left.resolve(device)?.as_mut().expect("Just resolved")
            };

            let MemTreeNode::Leave {
                files: left_files,
                dirty: left_dirty,
                lock: left_lock,
                ..
            } = left
            else {
                panic!("Expected to find leave");
            };

            left_lock.lock();

            if left_files.len() > left_files.capacity() / 2 {
                trace!("rotate leaves right");

                let last = left_files.remove(left_files.len() - 1);
                let new_max_id = left_files
                    .iter()
                    .last()
                    .expect("B-tree node should not be empty")
                    .id;
                *left_dirty = true;
                debug!("moved: {}", last.id.get());
                files.insert(0, last);

                unsafe {
                    // Safety: just locked above
                    left_lock.unlock();
                }

                children[node_index - 1].1.insert(new_max_id);

                unsafe {
                    // Safety: unbalanced_node and above is locked
                    self.unlock_upwards(unbalanced_node);
                }
                return Ok(());
            } else {
                unsafe {
                    // Safety: just locked above
                    left_lock.unlock();
                }
            }
        }

        if node_index + 1 < children.len() {
            let right = &mut children[node_index + 1].0;
            let right = unsafe {
                // Safety: parent is still locked
                right.resolve(device)?.as_mut().expect("Just resolved")
            };

            let MemTreeNode::Leave {
                files: right_files,
                dirty: right_dirty,
                lock: right_lock,
                ..
            } = right
            else {
                panic!("Expected to find leave");
            };

            right_lock.lock();

            if right_files.len() > right_files.capacity() / 2 {
                trace!("rotate leaves left");
                let first = right_files.remove(0);
                let first_file_id = first.id;
                *right_dirty = true;
                files.push(first);

                unsafe {
                    // Safety: just locked above
                    right_lock.unlock();
                }

                children[node_index].1.insert(first_file_id);

                unsafe {
                    // Safety: unbalanced_node and above is locked
                    self.unlock_upwards(unbalanced_node);
                }
                return Ok(());
            } else {
                unsafe {
                    // Safety: just locked above
                    right_lock.unlock();
                }
            }
        }

        trace!("merge leaves");

        #[allow(dropping_references)]
        {
            drop(files);
            drop(unbalanced_node);
        }

        let (left_index, right_index) = if node_index + 1 < children.len() {
            (node_index, node_index + 1)
        } else {
            (node_index - 1, node_index)
        };

        {
            let [mut left_link, mut right_link] = children
                .get_disjoint_mut([left_index, right_index])
                .expect("indices checked above");

            let left = unsafe {
                // Safety: parent is locked and old references are invalidated
                left_link.0.resolve(device)?.as_mut().expect("resolved")
            };
            let right = unsafe {
                // Safety: parent is locked and old references are invalidated
                right_link.0.resolve(device)?.as_mut().expect("resolved")
            };

            // one of left or right is already locked, because it points to the initial unbalanced_node
            let other_lock = if node_index == left_index {
                right.get_lock()
            } else {
                left.get_lock()
            };
            other_lock.lock();

            let MemTreeNode::Leave {
                parent: left_parent,
                files: left_files,
                dirty: left_dirty,
                ..
            } = left
            else {
                panic!("expected leave");
            };
            let MemTreeNode::Leave {
                parent: right_parent,
                files: right_files,
                ..
            } = right
            else {
                panic!("expected leave");
            };
            assert_eq!(left_parent, right_parent);

            // merge the nodes
            assert!(left_files.len() <= left_files.capacity() / 2);
            assert!(right_files.len() <= right_files.capacity() / 2);
            assert!(left_files.len() + right_files.len() == left_files.capacity() - 1);

            left_files.extend(right_files.drain(..));
            left_link.1 = right_link.1;

            unsafe {
                // a bit overkill as this is dropped later
                // Safety: Either right unbalanced_node or the other_lock. Both or locked
                right.get_lock().unlock();
            }

            let _old_link = children.remove(right_index);
            todo_warn!("We need to somehow remember to free the on device block");
        }

        if let Some(parent_parent) = parent_parent {
            assert!(children.len() >= children.capacity() / 2);

            if children.len() < children.capacity() / 2 {
                unsafe {
                    // Safety: parent is still locked
                    let merged_node = children[left_index]
                        .0
                        .as_mut()
                        .expect("node should be resolved by now");
                    let parent = merged_node.get_parent_mut().unwrap().as_mut();

                    // Safety: merged_node and above is locked
                    merged_node.get_lock().unlock();
                    // Safety: parent and above are locked
                    self.rebalance_node(parent, device)?;
                }
            }
        } else if children.len() == 1 {
            trace!("replace root with single leave!");
            let new_root_link = children.remove(0);
            assert!(new_root_link.1.is_none());
            let mut new_root_link = new_root_link.0;

            let new_root_node = unsafe {
                // Safety: parent is still locked
                new_root_link
                    .as_mut()
                    .expect("node should be resolved by now")
            };
            *new_root_node.get_parent_mut() = None;

            let old_root_link = unsafe {
                // Safety: root lock is still held
                self.get_root_mut()
            };

            let _old_root_link = core::mem::replace(old_root_link, new_root_link);
            todo_warn!("We need to somehow remember to free the on device block of the old root");

            unsafe {
                // Safety: reacquire and unlock root is save as it and root_lock is still locked
                let root_node = self
                    .get_root_mut()
                    .as_mut()
                    .expect("Root is still resolved");
                self.unlock_upwards(root_node);
            }

            return Ok(());
        }

        unsafe {
            // Safety: parent is still locked
            let merged_node = children[left_index]
                .0
                .as_mut()
                .expect("node should be resolved by now");
            // Safety: merged_node and above is locked
            self.unlock_upwards(merged_node);
        }

        Ok(())
    }

    /// Rebalances the tree using rotation and merges
    /// starting at a node
    ///
    /// # Safety
    ///
    /// the tree must be locked from this node upwards
    unsafe fn rebalance_node<D: BlockDevice>(
        &self,
        mut unbalanced_node: &mut MemTreeNode<I>,
        device: &D,
    ) -> Result<(), D::BlockDeviceError> {
        // TODO temp
        // unbalanced_node.assert_valid_node_only();

        let unbalanced_addr = unbalanced_node as *const _;
        let MemTreeNode::Node {
            parent,
            children: unbalanced_children,
            lock,
            ..
        } = unbalanced_node
        else {
            panic!("Expected node");
        };
        trace!("rebalance tree node");

        let mut parent = parent.expect("Expected non root node");
        let parent = unsafe {
            // Safety: the parent is locked and we have the only reference
            parent.as_mut()
        };
        let MemTreeNode::Node {
            children,
            parent: parent_parent,
            ..
        } = parent
        else {
            panic!("parent must always be a node");
        };

        let node_index = children
            .iter()
            .map(|c| c.0.node.as_ref().map(|node_box| Box::as_ptr(node_box)))
            .position(|c| c == Some(unbalanced_addr))
            .expect("parent should always contain it's children");

        if node_index >= 1 {
            let left = &mut children[node_index - 1].0;
            let left = unsafe {
                // Safety: parent is still locked
                left.resolve(device)?.as_mut().expect("Just resolved")
            };

            let MemTreeNode::Node {
                children: left_children,
                dirty: left_dirty,
                lock: left_lock,
                ..
            } = left
            else {
                panic!("Expected to find leave");
            };

            left_lock.lock();

            if left_children.len() > left_children.capacity() / 2 {
                trace!("rotate nodes right");
                let last = left_children.remove(left_children.len() - 1);
                let new_max_id = left_children
                    .iter_mut()
                    .last()
                    .expect("B-tree node should not be empty")
                    .1
                    .take()
                    .expect("second to last node in children should always have a max");
                *left_dirty = true;
                unbalanced_children.insert(0, last);

                unsafe {
                    // Safety: just locked above
                    left_lock.unlock();
                }

                children[node_index - 1].1.insert(new_max_id);

                unsafe {
                    // Safety: unbalanced_node and above is locked
                    self.unlock_upwards(unbalanced_node);
                }
                return Ok(());
            }
        }

        if node_index + 1 < children.len() {
            let right = &mut children[node_index + 1].0;
            let right = unsafe {
                // Safety: parent is still locked
                right.resolve(device)?.as_mut().expect("Just resolved")
            };

            let MemTreeNode::Node {
                children: right_children,
                dirty: right_dirty,
                lock: right_lock,
                ..
            } = right
            else {
                panic!("Expected to find leave");
            };

            right_lock.lock();

            if right_children.len() > right_children.capacity() / 2 {
                trace!("rotate nodes left");
                let first = right_children.remove(0);
                let first_node_max = first.1.expect("First node should always have a max value");
                *right_dirty = true;
                unbalanced_children.push(first);

                unsafe {
                    // Safety: just locked above
                    right_lock.unlock();
                }

                children[node_index].1.insert(first_node_max);

                unsafe {
                    // Safety: unbalanced_node and above is locked
                    self.unlock_upwards(unbalanced_node);
                }
                return Ok(());
            }
        }

        trace!("merge nodes");

        #[allow(dropping_references)]
        {
            drop(unbalanced_children);
            drop(unbalanced_node);
        }

        let (left_index, right_index) = if node_index + 1 < children.len() {
            (node_index, node_index + 1)
        } else {
            (node_index - 1, node_index)
        };

        {
            let [mut left_link, mut right_link] = children
                .get_disjoint_mut([left_index, right_index])
                .expect("indices checked above");

            let left = unsafe {
                // Safety: parent is locked and old references are invalidated
                left_link.0.resolve(device)?.as_mut().expect("resolved")
            };
            let right = unsafe {
                // Safety: parent is locked and old references are invalidated
                right_link.0.resolve(device)?.as_mut().expect("resolved")
            };

            // one of left or right is already locked, because it points to the initial unbalanced_node
            let other_lock = if node_index == left_index {
                right.get_lock()
            } else {
                left.get_lock()
            };
            other_lock.lock();

            let left_max = unsafe {
                // Safety: left is locked
                self.find_largest_id(left, device)?
                    .expect("This is never called on an empty root")
            };

            let MemTreeNode::Node {
                parent: left_parent,
                children: left_children,
                ..
            } = left
            else {
                panic!("expected node");
            };
            let left_len = left_children.capacity() / 2;
            assert_eq!(left_len, left_children.len());
            let MemTreeNode::Node {
                parent: right_parent,
                children: right_children,
                ..
            } = right
            else {
                panic!("expected node");
            };
            assert_eq!(left_parent, right_parent);

            left_children[left_len - 1].1 = Some(left_max);
            left_children.extend(right_children.drain(..));
            left_link.1 = right_link.1;

            unsafe {
                // a bit overkill as this is dropped later
                // Safety: Either right unbalanced_node or the other_lock. Both or locked
                right.get_lock().unlock();
            }

            let _old_link = children.remove(right_index);
            todo_warn!("We need to somehow remember to free the on device block");
        }

        assert!(children.len() >= children.capacity() / 2);
        if let Some(parent_parent) = parent_parent {
            if children.len() < children.capacity() / 2 {
                unsafe {
                    // Safety: parent is still locked
                    let merged_node = children[left_index]
                        .0
                        .as_mut()
                        .expect("node should be resolved by now");
                    let parent = merged_node.get_parent_mut().unwrap().as_mut();

                    // Safety: merged_node and above is locked
                    merged_node.get_lock().unlock();
                    // Safety: parent and above are locked
                    self.rebalance_node(parent, device)?;
                }
            }
        }

        unsafe {
            // Safety: parent is still locked
            let merged_node = children[left_index]
                .0
                .as_mut()
                .expect("node should be resolved by now");
            // Safety: merged_node and above is locked
            self.unlock_upwards(merged_node);
        }

        Ok(())
    }
    */

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

#[cfg(any(feature = "test", test))]
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
        files: StaticVec<Arc<FileNode>, LEAVE_MAX_FILE_COUNT>,
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

    fn has_dirty_leaves(&self) -> bool {
        match self {
            MemTreeNode::Node {
                dirty,
                dirty_children,
                ..
            } => *dirty || *dirty_children,
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

impl<I: InterruptState> MemTreeNode<I> {
    fn assert_valid_node_only(&self) {
        assert!(!self.get_lock().is_unlocked());

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
            }
            MemTreeNode::Leave { files, .. } => {
                assert!(files.iter().is_sorted_by_key(|f| f.id));
            }
        }
    }

    fn assert_valid(&self, parent: Option<&MemTreeNode<I>>) {
        let lock = self.get_lock();
        lock.lock();

        let mut rng_state: u32 = self.len() as u32;

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
/// pointers around that should no longer be accessed.
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
    fn drop_in_mem(&mut self, tree: &mut MemTree<I>) {
        let Some(node) = self.node.as_mut() else {
            return;
        };

        if node.has_dirty_leaves() {
            panic!("Can drop in mem representation of dirty sub-tree");
        }

        self.node = None;
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

#[multitest(cfg: feature = "test")]
mod test_mem_only {
    use alloc::{boxed::Box, sync::Arc, vec::Vec};
    use core::{
        assert_matches::assert_matches,
        num::NonZero,
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

    fn create_file_node(id: u64) -> Arc<FileNode> {
        Arc::new(FileNode {
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
        })
    }

    fn file_id(id: u64) -> FileId {
        FileId::try_new(id).unwrap()
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

    /*
    #[kernel_test]
    fn test_delete_no_rebalance() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(1), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(2), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(3), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(4), true)
            .tunwrap()?;

        tree.assert_valid_full();

        tree.delete(&TestBlockDevice, file_id(3)).tunwrap()?;

        tree.assert_valid_full();

        let root = unsafe {
            // Safety: we have unique access right now and can ignore the lock
            tree.get_root().as_ref().unwrap()
        };

        if let MemTreeNode::Leave { files, .. } = root {
            t_assert_eq!(
                &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
                &[1, 2, 4]
            );
        } else {
            tfail!("Expected to find a leave node at tree root");
        }

        Ok(())
    }

    #[kernel_test]
    fn test_delete_unbalance_root() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(1), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(3), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(4), true)
            .tunwrap()?;

        tree.assert_valid_full();

        tree.delete(&TestBlockDevice, file_id(3)).tunwrap()?;

        tree.assert_valid_full();

        let root = unsafe {
            // Safety: we have unique access right now and can ignore the lock
            tree.get_root().as_ref().unwrap()
        };

        if let MemTreeNode::Leave { files, .. } = root {
            t_assert_eq!(
                &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
                &[1, 4]
            );
        } else {
            tfail!("Expected to find a leave node at tree root");
        }

        Ok(())
    }

    #[kernel_test]
    fn test_delete_in_leave() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(1), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(2), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(3), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(4), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(5), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(6), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(7), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(8), true)
            .tunwrap()?;

        tree.assert_valid_full();

        tree.delete(&TestBlockDevice, file_id(3)).tunwrap()?;

        tree.assert_valid_full();

        let mut nodes = tree.iter_nodes();

        let _root = nodes.next();

        let Some(MemTreeNode::Leave { files, .. }) = nodes.next() else {
            tfail!("Expected leave");
        };
        t_assert_eq!(
            &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
            &[1, 2, 4]
        );

        let Some(MemTreeNode::Leave { files, .. }) = nodes.next() else {
            tfail!("Expected leave");
        };
        t_assert_eq!(
            &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
            &[5, 6, 7, 8]
        );

        t_assert!(nodes.next().is_none());

        Ok(())
    }

    #[kernel_test]
    fn test_delete_rotate_left() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(1), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(2), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(3), true)
            .tunwrap()?;

        tree.insert(&TestBlockDevice, create_file_node(5), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(6), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(7), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(8), true)
            .tunwrap()?;

        tree.assert_valid_full();

        tree.delete(&TestBlockDevice, file_id(3)).tunwrap()?;

        tree.assert_valid_full();

        let mut nodes = tree.iter_nodes();

        let _root = nodes.next();

        let Some(MemTreeNode::Leave { files, .. }) = nodes.next() else {
            tfail!("Expected leave");
        };
        t_assert_eq!(
            &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
            &[1, 2, 5]
        );

        let Some(MemTreeNode::Leave { files, .. }) = nodes.next() else {
            tfail!("Expected leave");
        };
        t_assert_eq!(
            &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
            &[6, 7, 8]
        );

        t_assert!(nodes.next().is_none());

        Ok(())
    }

    #[kernel_test]
    fn test_delete_rotate_right() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(1), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(2), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(3), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(4), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(6), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(7), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(8), true)
            .tunwrap()?;

        tree.assert_valid_full();

        tree.delete(&TestBlockDevice, file_id(7)).tunwrap()?;

        tree.assert_valid_full();

        let mut nodes = tree.iter_nodes();

        let _root = nodes.next();

        let Some(MemTreeNode::Leave { files, .. }) = nodes.next() else {
            tfail!("Expected leave");
        };
        t_assert_eq!(
            &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
            &[1, 2, 3]
        );

        let Some(MemTreeNode::Leave { files, .. }) = nodes.next() else {
            tfail!("Expected leave");
        };
        t_assert_eq!(
            &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
            &[4, 6, 8]
        );

        t_assert!(nodes.next().is_none());

        Ok(())
    }

    #[kernel_test]
    fn test_delete_merge() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(1), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(2), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(3), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(4), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(6), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(7), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(8), true)
            .tunwrap()?;

        tree.assert_valid_full();

        tree.delete(&TestBlockDevice, file_id(7)).tunwrap()?;
        tree.delete(&TestBlockDevice, file_id(3)).tunwrap()?;

        tree.assert_valid_full();

        let mut nodes = tree.iter_nodes();

        if let Some(MemTreeNode::Leave { files, .. }) = nodes.next() {
            t_assert_eq!(
                &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
                &[1, 2, 4, 6, 8]
            );
        } else {
            tfail!("Expected to find a leave node at tree root");
        }

        t_assert!(nodes.next().is_none());

        Ok(())
    }

    #[kernel_test]
    fn test_delete_merge_last() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        tree.insert(&TestBlockDevice, create_file_node(1), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(2), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(3), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(4), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(6), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(7), true)
            .tunwrap()?;
        tree.insert(&TestBlockDevice, create_file_node(8), true)
            .tunwrap()?;

        tree.assert_valid_full();

        tree.delete(&TestBlockDevice, file_id(3)).tunwrap()?;
        tree.delete(&TestBlockDevice, file_id(7)).tunwrap()?;

        tree.assert_valid_full();

        let mut nodes = tree.iter_nodes();

        if let Some(MemTreeNode::Leave { files, .. }) = nodes.next() {
            t_assert_eq!(
                &files.iter().map(|f| f.id.get()).collect::<Vec<_>>(),
                &[1, 2, 4, 6, 8]
            );
        } else {
            tfail!("Expected to find a leave node at tree root");
        }

        Ok(())
    }

    #[kernel_test(f)]
    fn test_delete_100_files() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

        unsafe {
            NonZero::<u64>::new_unchecked(5);
        }

        let insert_ids: &[u64] = &[
            42, 67, 13, 91, 28, 73, 5, 38, 84, 19, 99, 7, 34, 53, 22, 88, 45, 96, 31, 11, 79, 62,
            16, 55, 3, 94, 49, 81, 26, 50, 72, 9, 39, 100, 63, 29, 86, 47, 14, 97, 1, 66, 24, 57,
            78, 36, 95, 8, 58, 92, 33, 48, 74, 12, 30, 98, 61, 21, 46, 64, 82, 23, 40, 90, 10, 37,
            59, 76, 20, 6, 35, 56, 68, 80, 27, 87, 2, 70, 17, 51, 60, 44, 32, 25, 77, 54, 85, 4,
            15, 93, 43, 18, 41, 52, 83, 65, 71, 89, 75, 69,
        ];

        let delete_ids: &[u64] = &[
            27, 61, 45, 88, 74, 99, 12, 33, 6, 56, 91, 39, 18, 85, 23, 64, 77, 14, 52, 94, 8, 47,
            31, 100, 3, 20, 71, 58, 43, 79, 29, 96, 66, 10, 36, 26, 55, 82, 95, 5, 70, 28, 15, 53,
            7, 97, 86, 48, 37, 92, 87, 41, 81, 90, 32, 1, 21, 50, 11, 67, 83, 24, 76, 46, 44, 16,
            80, 22, 60, 4, 35, 25, 34, 30, 59, 49, 2, 40, 78, 68, 9, 72, 63, 17, 19, 84, 13, 42,
            98, 54, 93, 89, 57, 75, 51, 65, 38, 73, 69, 62,
        ];

        let mut files = Vec::new();

        for id in insert_ids {
            let file = create_file_node(*id);
            files.push(file.clone());
            tree.insert(&TestBlockDevice, file, true).tunwrap()?;
        }
        tree.assert_valid_full();

        for id in delete_ids {
            trace!("Delete {id}");
            // TODO temp
            if *id == 69 {
                log::warn!("fucked up");
                // super::BORK.store(true, core::sync::atomic::Ordering::SeqCst);
            }
            tree.delete(&TestBlockDevice, file_id(*id)).tunwrap()?;

            for f in &files {
                if f.id.get() > 100 {
                    log::error!(
                        "File ids: {:?}",
                        files.iter().map(|f| f.id.get()).collect::<Vec<_>>()
                    );
                    tfail!("broken after delete {id}");
                }
            }
            if *id == 73 {
                log::warn!("fucked up");
            }
            tree.assert_valid_full();

            for f in &files {
                if f.id.get() > 100 {
                    tfail!(
                        "File ids: {:?}",
                        files.iter().map(|f| f.id.get()).collect::<Vec<_>>()
                    );
                    tfail!("broken after valid check {id}");
                }
            }
        }
        for f in &files {
            if f.id.get() > 100 {
                tfail!(
                    "File ids: {:?}",
                    files.iter().map(|f| f.id.get()).collect::<Vec<_>>()
                );
                tfail!();
            }
        }
        debug!("Delete done");
        tree.assert_valid_full();

        let root = unsafe {
            // Safety: we have unique access right now and can ignore the lock
            tree.get_root().as_ref().unwrap()
        };

        if let MemTreeNode::Leave { files, .. } = root {
            t_assert_eq!(files.len(), 0);
        } else {
            tfail!("Expected to find a leave node at tree root");
        }

        Ok(())
    }
    */
}
