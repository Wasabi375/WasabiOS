use core::{
    alloc::AllocError,
    assert_matches::assert_matches,
    borrow::BorrowMut,
    cell::{RefCell, UnsafeCell},
    default, error,
    error::Error,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    ops::Deref,
    panic,
    ptr::{self, NonNull, null_mut},
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use alloc::{
    boxed::{self, Box},
    sync::Arc,
    vec::Vec,
};
use log::trace;
use shared::{
    sync::{InterruptState, lockcell::UnsafeTicketLock},
    todo_error, todo_warn,
    r#unsafe::extend_lifetime_mut,
};
use staticvec::{StaticVec, staticvec};
use testing_derive::multitest;
use thiserror::Error;

use crate::{
    Block,
    block_allocator::BlockAllocator,
    fs::{FsError, map_device_error},
    fs_structs::{
        DevicePointer, FileId, FileNode, LEAVE_MAX_FILE_COUNT, MainHeader, NODE_MAX_CHILD_COUNT,
        TreeNode,
    },
    interface::BlockDevice,
};

use self::iter::MemTreeNodeIter;

pub mod iter;
mod node_guard;

use node_guard::NodeGuard;

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum MemTreeError {
    #[error("Out of Memory")]
    Oom(#[from] AllocError),
    #[error("Block Device Failure: {0}")]
    BlockDevice(Box<dyn Error + Send + Sync>),
    #[error("FileNode with id {0} already exists")]
    FileNodeExists(FileId),
    #[error("FileNode with id {0} does not exist")]
    FileDoesNotExist(FileId),
}

impl MemTreeError {
    pub fn from<E: Error + Send + Sync + 'static>(value: E) -> Self {
        Self::BlockDevice(Box::new(value))
    }
}

/// An in memory view of the on device file tree
///
/// Any modification of the file tree is done on this data structure and later saved to the device
/// using a copy on write system, meaning that the [MainHeader::root] will always point to a
/// complete and valid file tree.
///
/// TODO: how do I ensure that old "used" blocks stay around until the main root is updated on
/// device
///
/// # Data Structure
///
/// Both the on device tree([TreeNode]) and the in memory view are represented as a [B+Tree] or to
/// be more exact a [B-Tree] where the files are stored in the leaves. The keys in internal nodes
/// instead represent the largest key of the corresponding subtree.
///
/// ## Locking
///
/// This implementation uses fine grained locking. The idea is that each node in the tree has it's
/// own lock. The system by which locks are acquired depend on whether the operation is readonly or
/// is read/write.
///
/// For readonly operation only a few locks are held at a time. First the `root_lock` is acquired.
/// This allows to read the root node and lock that. After the lock for a node is acquired the node
/// for the parent (or the `root_lock`) is unlocked again. This way only 2 locks are held at the
/// same time. The lock for the current level node and the lock for the next level. This allows for
/// good parallelization as chances are that 2 operations will access separate subtrees.
///
/// For read/write operations all locks from the `root_lock` down to the leave are locked and held
/// until the operation finishes. Locking works similar to readonly operations but the parent's
/// lock is not released. Read/write operations release the locks from the leave upwards as nodes
/// are no longer accessed.
///
/// Locking nodes from the root down to the leaves allows multiple read operations to run in
/// parallel. This allso allows to start a write operation while reads are still processing. Reads
/// will have to wait for any write operation to finish however, because write operations don't
/// release the `root_lock` until they are finished.
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
}

impl<I> !Clone for MemTree<I> {}

impl<I: InterruptState> MemTree<I> {
    /// Creates a new empty [MemTree]
    pub fn empty(device_ptr: Option<DevicePointer<TreeNode>>) -> Self {
        let root_node = MemTreeNode::Leave {
            parent: None,
            files: StaticVec::new(),
            dirty: true,
            lock: UnsafeTicketLock::new(),
        };

        let root = MemTreeLink {
            node: Some(Box::new(root_node)),
            device_ptr,
        };

        MemTree {
            root: root.into(),
            root_lock: UnsafeTicketLock::new(),
        }
    }

    /// Creates a new [MemTree] that is an invalid state.
    ///
    /// In particular the root node is neither available in mem nor on device,
    /// which leads to a panic if accessed.
    pub fn invalid() -> Self {
        let root = MemTreeLink {
            node: None,
            device_ptr: None,
        };

        MemTree {
            root: root.into(),
            root_lock: UnsafeTicketLock::new(),
        }
    }

    /// Sets the device ptr for the root node
    ///
    /// This panics if the root node is availabe either in memory or on device.
    /// Should only be called if `self` was created using [Self::invalid]
    pub fn set_root_device_ptr(&mut self, root_device_ptr: DevicePointer<TreeNode>) {
        let root = unsafe {
            // Safety: we have mut access to self therefor we don't need the lock
            &mut self.get_root_mut()
        };

        assert!(root.device_ptr.is_none());
        assert!(root.node.is_none());

        root.device_ptr = Some(root_device_ptr);
    }

    /// Gets the root node
    ///
    /// # Safety
    ///
    /// The caller muset ensure the root_lock is held and no mut ref
    /// to the root currently exists
    unsafe fn get_root(&self) -> &MemTreeLink<I> {
        unsafe { &*self.root.get() }
    }

    /// Gets the root node
    ///
    /// # Safety
    ///
    /// The caller muset ensure the root_lock is held and no mut ref
    /// to the root currently exists
    #[allow(clippy::mut_from_ref)]
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
    ) -> Result<NodeGuard<'_, I, BorrowType>, FsError> {
        self.root_lock.lock();

        let mut current_node = unsafe {
            // Safety: we hold the root lock
            self.get_root_mut().resolve(device, None)?.as_mut()
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
            // Safety:
            // The current_node_ptr is created from a shared reference.
            // Using a mutalbe reference would move out of current_node as &mut T is never Copy
            // This is save as the current_node_ptr is only cast back to a reference after
            // this reference here no longer exists.
            let current_node_ptr = Some(NonNull::from(current_node as &_));
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
                            child.resolve(device, current_node_ptr)?;
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
                leave @ MemTreeNode::Leave { .. } => unsafe {
                    // Safety: leave is properly locked
                    // and we still hold all parent locks for BorrowType::is_mut
                    return Ok(NodeGuard::new(leave));
                },
            }
        }
    }

    /// Unlocks node and all above nodes as well as the root_lock
    fn unlock_upwards(&self, node: NodeGuard<'_, I, node_guard::Mut>) {
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
    ) -> Result<Option<Arc<FileNode>>, FsError> {
        let leave = self.find_leave::<_, node_guard::Immut>(device, id)?;
        let MemTreeNode::Leave { files, lock, .. } = leave.borrow() else {
            panic!("Find leave should always return a leave node");
        };

        let result = files.iter().find(|f| f.id == id).cloned();

        Ok(result)
    }

    pub fn create<D: BlockDevice>(
        &self,
        device: &D,
        file: Arc<FileNode>,
    ) -> Result<(), MemTreeError> {
        self.insert(device, file, true)
    }

    pub fn update<D: BlockDevice>(
        &self,
        device: &D,
        file: Arc<FileNode>,
    ) -> Result<(), MemTreeError> {
        self.insert(device, file, false)
    }

    fn insert<D: BlockDevice>(
        &self,
        device: &D,
        file: Arc<FileNode>,
        create_only: bool,
    ) -> Result<(), MemTreeError> {
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
                self.unlock_upwards(leave);

                return Err(MemTreeError::FileNodeExists(file.id));
            }

            *dirty = true;
            files[override_pos] = file;

            self.unlock_upwards(leave);

            return Ok(());
        }

        *dirty = true;

        let insert_pos = files
            .iter()
            .position(|f| f.id > file.id)
            .unwrap_or(files.len());

        if files.is_not_full() {
            files.insert(insert_pos, file);

            self.unlock_upwards(leave);

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
            parent: *parent,
            files: right_node_files,
            dirty: true,
            lock: UnsafeTicketLock::new(),
        };

        match leave.into_parent() {
            Ok((parent, leave_ptr)) => {
                self.insert_split(parent, leave_ptr, split_id, new_node, new_node_max)
            }
            Err(root) => self.insert_split_at_root(root, split_id, new_node),
        }
    }

    /// Insert the `new_node` that was created by a split in the parent
    ///
    /// TODO document args
    ///
    fn insert_split(
        &self,
        mut current: NodeGuard<'_, I, node_guard::Mut>,
        old_node_ptr: NonNull<MemTreeNode<I>>,
        old_node_new_max: FileId,
        new_node: MemTreeNode<I>,
        new_node_max: FileId,
    ) -> Result<(), MemTreeError> {
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

            self.unlock_upwards(current);

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
            parent: *next_parent,
            children: right_node_children,
            dirty: true,
            dirty_children: true,
            lock: UnsafeTicketLock::new(),
        };

        match current.into_parent() {
            Ok((parent, current_ptr)) => {
                self.insert_split(parent, current_ptr, split_id, new_node, new_node_max)
            }
            Err(root) => self.insert_split_at_root(root, split_id, new_node),
        }
    }

    /// Insert the `new_node` that was created by a split at the root level
    ///
    /// `root` should be the tree root and `new_node` should be the new right
    /// subtree after the slpit.
    fn insert_split_at_root(
        &self,
        root: NodeGuard<'_, I, node_guard::Mut>,
        split_id: FileId,
        new_node: MemTreeNode<I>,
    ) -> Result<(), MemTreeError> {
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
                .map(Box::as_ptr)
                .expect("root node should be some");

            assert_eq!(old_root_link_node_ptr, root_node_ptr);

            (old_root_link_node, old_root_link.device_ptr.take())
        };

        let left_link = MemTreeLink {
            node: old_root_link_node,
            device_ptr: old_root_link_device,
        };

        let right_node = Box::try_new(new_node)?;
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
            unreachable!() // TODO: why?
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

    pub fn delete<D: BlockDevice>(
        &self,
        device: &D,
        file_id: FileId,
    ) -> Result<Arc<FileNode>, MemTreeError> {
        // NOTE: this will have to resolve on-device only nodes because we want to delete from the
        // device and not just the in memory FileNode. Therefor in order to keep the tree balanced
        // we have to find the file even if it is not in memory. Therefor it is ok to use
        // find_leave
        let mut leave = self
            .find_leave::<_, node_guard::Mut>(device, file_id)
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

        let Some(file_pos) = files.iter().position(|f| f.id == file_id) else {
            self.unlock_upwards(leave);
            return Err(MemTreeError::FileDoesNotExist(file_id));
        };

        let file_node = files.remove(file_pos);
        *dirty = true;

        if files.len() >= files.capacity() / 2 || parent.is_none() {
            self.unlock_upwards(leave);
            return Ok(file_node);
        }

        if parent.is_some() {
            self.rebalance_leave(leave, device)?;
        }

        Ok(file_node)
    }

    /// Rebalances the tree using rotation and merges
    /// starting at a leave
    #[inline(always)]
    fn rebalance_leave<D: BlockDevice>(
        &self,
        unbalanced_node: NodeGuard<'_, I, node_guard::Mut>,
        device: &D,
    ) -> Result<(), MemTreeError> {
        let (mut parent, unbalanced_node_ptr) = unbalanced_node
            .into_parent()
            .expect("Rebalance leave should only be called for nodes with parents");

        let ChildrenForRebalance {
            mut left_guard,
            left_max_id,
            mut right_guard,
            right_max_id,
            mode,
        } = self
            .get_children_for_rebalance(&mut parent, unbalanced_node_ptr, device)
            .map_err(MemTreeError::from)?;

        let left = left_guard.borrow_mut();
        let right = right_guard.borrow_mut();

        let MemTreeNode::Leave {
            parent: parent_ptr,
            files: left_files,
            dirty: left_dirty,
            ..
        } = left
        else {
            panic!("all nodes on this level should be leaves");
        };
        let MemTreeNode::Leave {
            files: right_files,
            dirty: right_dirty,
            ..
        } = right
        else {
            panic!("all nodes on this level should be leaves");
        };

        *left_dirty = true;
        *right_dirty = true;

        match mode {
            DeleteRebalanceMode::TakeFromRight => {
                let to_move = right_files.remove(0);
                assert!(right_files.len() >= right_files.capacity() / 2);

                assert!(
                    to_move.id
                        > left_max_id.expect(
                            "max id is only none for the right most node, and this is the left node"
                        )
                );
                *left_max_id = Some(to_move.id);
                left_files.push(to_move);
            }
            DeleteRebalanceMode::TakeFromLeft => {
                let to_move = left_files.remove(left_files.len() - 1);
                assert!(left_files.len() >= left_files.capacity() / 2);

                *left_max_id = Some(left_files[left_files.len() - 1].id);

                assert!(to_move.id < right_files[0].id);
                right_files.insert(0, to_move);
            }
            DeleteRebalanceMode::Merge { merge_left_index } => {
                let mut merged_files = left_files.clone();
                merged_files.extend(right_files.drain_iter(..));

                let merged_max_id = *right_max_id;

                let new_node = Box::try_new(MemTreeNode::Leave {
                    parent: *parent_ptr,
                    files: merged_files,
                    dirty: true,
                    lock: UnsafeTicketLock::new(),
                })
                .map_err(|err| MemTreeError::Oom(err))?;

                let new_link = MemTreeLink {
                    node: Some(new_node),
                    device_ptr: None,
                };

                drop(left_guard);
                drop(right_guard);

                let MemTreeNode::Node { children, .. } = parent.borrow_mut() else {
                    unreachable!("Parents are always non leave nodes");
                };

                todo_warn!(
                    "what to do with the now no longer used on device nodes? How do I store them for later deletion"
                );

                children[merge_left_index] = (new_link, merged_max_id);
                children.remove(merge_left_index + 1);

                return self.rebalance_node(parent, device);
            }
        }

        drop(left_guard);
        drop(right_guard);

        self.unlock_upwards(parent);
        Ok(())
    }

    /// Rebalances the tree using rotation and merges
    /// starting at a leave
    #[inline(always)]
    fn rebalance_node<D: BlockDevice>(
        &self,
        unbalanced_node: NodeGuard<'_, I, node_guard::Mut>,
        device: &D,
    ) -> Result<(), MemTreeError> {
        let (mut parent, unbalanced_node_ptr) = match unbalanced_node.into_parent() {
            Ok((parent, unbalanced_node_ptr)) => (parent, unbalanced_node_ptr),
            Err(mut unbalanced_root_guard) => {
                let unbalanced_root = unbalanced_root_guard.borrow_mut();

                if unbalanced_root.len() == 1 {
                    // Need to awaken the unbalanced root node, as we invalidate it when
                    // we promote it's only child to root
                    let unbalanced_root = unbalanced_root_guard.awaken_mut();

                    let MemTreeNode::Node { children, .. } = unbalanced_root else {
                        panic!("all nodes on this level should be non leave nodes");
                    };

                    assert_eq!(children.len(), 1);
                    let (mut new_root_link, max) = children.remove(0);

                    assert!(max.is_none());

                    let new_root = unsafe {
                        // Safety: we hold the parents lock
                        new_root_link.as_mut()
                    }
                    .expect("this node should have been created/modified in a previous rebalance");

                    new_root.get_lock().lock();

                    *new_root.get_parent_mut() = None;

                    let _old_root_device_ptr = {
                        let old_root_link = unsafe {
                            // Safety: we still hold the root lock, because we have a mut node
                            // guard
                            self.get_root()
                        };
                        old_root_link.device_ptr
                    };

                    todo_warn!("what to do with the old root device ptr. How to delete");

                    unsafe {
                        // We manually locked this above
                        new_root.get_lock().unlock();

                        // Safety: we still hold the root lock, because we have a mut node guard
                        *self.get_root_mut() = new_root_link;

                        // Safety: we are responsible for the root lock because we called
                        // `awaken_mut` on the old root.
                        self.root_lock.unlock();
                    }
                } else {
                    self.unlock_upwards(unbalanced_root_guard);
                }
                return Ok(());
            }
        };

        let ChildrenForRebalance {
            mut left_guard,
            left_max_id,
            mut right_guard,
            right_max_id,
            mode,
        } = self
            .get_children_for_rebalance(&mut parent, unbalanced_node_ptr, device)
            .map_err(MemTreeError::from)?;

        let left = left_guard.borrow_mut();
        let right = right_guard.borrow_mut();

        let MemTreeNode::Node {
            parent: parent_ptr,
            children: left_children,
            dirty: left_dirty,
            ..
        } = left
        else {
            panic!("all nodes on this level should be non leave nodes");
        };
        let MemTreeNode::Node {
            children: right_children,
            dirty: right_dirty,
            ..
        } = right
        else {
            panic!("all nodes on this level should be non leave nodes");
        };

        *left_dirty = true;
        *right_dirty = true;

        match mode {
            DeleteRebalanceMode::TakeFromRight => {
                let to_move = right_children.remove(0);
                assert!(right_children.len() >= right_children.capacity() / 2);

                let left_new_max = to_move
                    .1
                    .expect("This is the left most node, therefor should have the max set");
                assert!(
                    left_new_max
                        > left_max_id.expect(
                            "max id is only none for the right most node, and this is the left node"
                        )
                );
                *left_max_id = Some(left_new_max);

                left_children.push(to_move);
            }
            DeleteRebalanceMode::TakeFromLeft => {
                let mut to_move = left_children.remove(left_children.len() - 1);
                assert!(left_children.len() >= left_children.capacity() / 2);

                assert!(to_move.1.is_none());
                // the max of the parent's link is the max of the "to_move" node.
                // As the "to_move" node is the last node it's max is set to none currently.
                // But because it is inserted as the 0th node the max has to be set now.
                to_move.1 = Some(
                    left_max_id
                        .expect("This is the left most node, therefor should have the max set"),
                );
                let new_last_left = {
                    let last_id = left_children.len() - 1;
                    &mut left_children[last_id]
                };

                // take because the now last element needs to have None as the max.
                // I want to move it into the max in the parent link
                let new_left_max = new_last_left
                    .1
                    .take()
                    .expect("This was the second last node, and therefor should be set");

                *left_max_id = Some(new_left_max);

                right_children.insert(0, to_move);
            }
            DeleteRebalanceMode::Merge { merge_left_index } => {
                let mut merged_children = StaticVec::new();
                merged_children.extend(left_children.drain(..));
                merged_children.extend(right_children.drain(..));

                let merged_max_id = *right_max_id;

                let new_node = Box::try_new(MemTreeNode::Node {
                    parent: *parent_ptr,
                    children: merged_children,
                    dirty: true,
                    dirty_children: true,
                    lock: UnsafeTicketLock::new(),
                })
                .map_err(|err| MemTreeError::Oom(err))?;

                let new_link = MemTreeLink {
                    node: Some(new_node),
                    device_ptr: None,
                };

                drop(left_guard);
                drop(right_guard);

                let MemTreeNode::Node { children, .. } = parent.borrow_mut() else {
                    unreachable!("Parents are always non leave nodes");
                };

                todo_warn!(
                    "what to do with the now no longer used on device nodes? How do I store them for later deletion"
                );

                children[merge_left_index] = (new_link, merged_max_id);
                children.remove(merge_left_index + 1);

                return self.rebalance_node(parent, device);
            }
        }

        drop(left_guard);
        drop(right_guard);

        self.unlock_upwards(parent);
        Ok(())
    }

    fn get_children_for_rebalance<'l, 'n: 'l, D: BlockDevice, B: node_guard::BorrowMut>(
        &self,
        node: &'l mut NodeGuard<'n, I, B>,
        unbalanced_ptr: NonNull<MemTreeNode<I>>,
        device: &D,
    ) -> Result<ChildrenForRebalance<'l, I>, FsError> {
        let current_node_ptr = node.as_ptr();

        let MemTreeNode::Node {
            children, parent, ..
        } = node.borrow_mut()
        else {
            panic!("get_children_for_rebalance should only be called for non leave nodes");
        };

        assert!(
            children.len() > children.capacity() / 2 || parent.is_none(),
            "Self should be balanced or root"
        );
        assert!(
            children.len() >= 2,
            "Even if root we should have at least 2 children" // if not, that means the node has 1 child (the unbalanced node) meaning
                                                              // the unbalanced node should have beeen promoted to root in the past
        );

        let unbalanced_index = children
            .iter()
            .position(|c| {
                let link = &c.0;
                if let Some(node) = link.node.as_ref() {
                    let ptr = Box::as_ptr(node);

                    core::ptr::eq(ptr, unbalanced_ptr.as_ptr())
                } else {
                    false
                }
            })
            .expect("unbalanced ptr should point into self");

        enum RebalanceVariants {
            Merge(usize, usize),
            TakeFromRight,
            TakeFromLeft,
        }

        // variant, rebalance
        let to_check: &[_] = if unbalanced_index == 0 {
            use RebalanceVariants::*;
            &[
                (TakeFromRight, false),
                (Merge(unbalanced_index, unbalanced_index + 1), false),
                (Merge(unbalanced_index, unbalanced_index + 1), true),
                (TakeFromRight, true),
            ]
        } else if unbalanced_index == children.len() - 1 {
            use RebalanceVariants::*;
            &[
                (TakeFromLeft, false),
                (Merge(unbalanced_index - 1, unbalanced_index), false),
                (Merge(unbalanced_index - 1, unbalanced_index), true),
                (TakeFromLeft, true),
            ]
        } else {
            use RebalanceVariants::*;
            &[
                (TakeFromRight, false),
                (TakeFromLeft, false),
                (Merge(unbalanced_index - 1, unbalanced_index), false),
                (Merge(unbalanced_index, unbalanced_index + 1), false),
                (Merge(unbalanced_index - 1, unbalanced_index), true),
                (Merge(unbalanced_index, unbalanced_index + 1), true),
                (TakeFromRight, true),
                (TakeFromLeft, true),
            ]
        };

        for (variante, resolve) in to_check {
            match variante {
                RebalanceVariants::Merge(left, right) => {
                    let merge_left_index = *left;
                    let [(left_link, left_max), (right_link, right_max)] =
                        children.get_disjoint_mut([*left, *right]).unwrap();

                    if *resolve {
                        left_link.resolve(device, Some(current_node_ptr))?;
                        right_link.resolve(device, Some(current_node_ptr))?;
                    }
                    let Some(left) = (unsafe {
                        // Safety: we have the guard for the parent
                        left_link.as_mut()
                    }) else {
                        continue;
                    };
                    let Some(right) = (unsafe {
                        // Safety: we have the guard for the parent
                        right_link.as_mut()
                    }) else {
                        continue;
                    };

                    if left.len() + right.len() <= left.cap() {
                        left.get_lock().lock();
                        right.get_lock().lock();

                        unsafe {
                            // Safety: extending the lifetime is safe here, because we can borrow
                            // from children as 'l.
                            // This is only valid because we either borrow just for this iteration
                            // of the loop or return the reference, therefor ending the loop.
                            // Extend can't be used to keep a reference into the next iteration of
                            // the loop
                            return Ok(ChildrenForRebalance {
                                // Safety: we just locked this
                                left_guard: NodeGuard::new(extend_lifetime_mut(left)),
                                left_max_id: extend_lifetime_mut(left_max),
                                // Safety: we just locked this
                                right_guard: NodeGuard::new(extend_lifetime_mut(right)),
                                right_max_id: extend_lifetime_mut(right_max),
                                mode: DeleteRebalanceMode::Merge { merge_left_index },
                            });
                        }
                    }
                }
                RebalanceVariants::TakeFromRight => {
                    let [(left_link, left_max), (right_link, right_max)] = children
                        .get_disjoint_mut([unbalanced_index, unbalanced_index + 1])
                        .unwrap();

                    if *resolve {
                        right_link.resolve(device, Some(current_node_ptr))?;
                    }

                    let left = unsafe {
                        // Safety: we have the guard for the parent
                        left_link
                            .as_mut()
                            .expect("unbalanced node should always be resolved")
                    };
                    let Some(right) = (unsafe {
                        // Safety: we have the guard for the parent
                        right_link.as_mut()
                    }) else {
                        continue;
                    };

                    if right.len() > right.cap() / 2 {
                        left.get_lock().lock();
                        right.get_lock().lock();

                        unsafe {
                            // Safety: extending the lifetime is safe here, because we can borrow
                            // from children as 'l.
                            // This is only valid because we either borrow just for this iteration
                            // of the loop or return the reference, therefor ending the loop.
                            // Extend can't be used to keep a reference into the next iteration of
                            // the loop
                            return Ok(ChildrenForRebalance {
                                // Safety: we just locked this
                                left_guard: NodeGuard::new(extend_lifetime_mut(left)),
                                left_max_id: extend_lifetime_mut(left_max),
                                // Safety: we just locked this
                                right_guard: NodeGuard::new(extend_lifetime_mut(right)),
                                right_max_id: extend_lifetime_mut(right_max),
                                mode: DeleteRebalanceMode::TakeFromRight,
                            });
                        }
                    }
                }
                RebalanceVariants::TakeFromLeft => {
                    let [(left_link, left_max), (right_link, right_max)] = children
                        .get_disjoint_mut([unbalanced_index - 1, unbalanced_index])
                        .unwrap();

                    if *resolve {
                        left_link.resolve(device, Some(current_node_ptr))?;
                    }

                    let Some(left) = (unsafe {
                        // Safety: we have the guard for the parent
                        left_link.as_mut()
                    }) else {
                        continue;
                    };
                    let right = unsafe {
                        // Safety: we have the guard for the parent
                        right_link
                            .as_mut()
                            .expect("unbalanced node should always be resolved")
                    };

                    if left.len() > left.cap() / 2 {
                        left.get_lock().lock();
                        right.get_lock().lock();

                        unsafe {
                            // Safety: extending the lifetime is safe here, because we can borrow
                            // from children as 'l.
                            // This is only valid because we either borrow just for this iteration
                            // of the loop or return the reference, therefor ending the loop.
                            // Extend can't be used to keep a reference into the next iteration of
                            // the loop
                            return Ok(ChildrenForRebalance {
                                // Safety: we just locked this
                                left_guard: NodeGuard::new(extend_lifetime_mut(left)),
                                left_max_id: extend_lifetime_mut(left_max),
                                // Safety: we just locked this
                                right_guard: NodeGuard::new(extend_lifetime_mut(right)),
                                right_max_id: extend_lifetime_mut(right_max),
                                mode: DeleteRebalanceMode::TakeFromLeft,
                            });
                        }
                    }
                }
            }
        }

        panic!("No valid rebalance variance found. This should never happen for a valid tree");
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

    pub fn flush_to_device<D: BlockDevice>(
        &self,
        device: &mut D,
        block_allocator: &mut BlockAllocator,
    ) -> Result<(), FsError> {
        trace!("flush");
        self.root_lock.lock();

        let root_link = unsafe {
            // Safety: we hold the root lock
            self.get_root_mut()
        };

        root_link.flush_to_device(None, device, block_allocator)?;

        unsafe {
            // Safety: we locked this ourself
            self.root_lock.unlock();
        }
        Ok(())
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

// NOTE MemTreeNode is nearly always stored in a box, also the
// capacity of children/files is dependent on the space on
// disk, not in mem.
// I don't want to use Vec, because I don't want the extra
// indirection and also having the capacity available
// independent of leave/node is usefull
#[allow(clippy::large_enum_variant)]
/// A node within a [MemTree]
///
/// This is either a `Leave` storing [FileNode] data or an internal `Node`
/// which connect the leaves into a B-Tree like structure.
///
/// This is the in memory representation of a [TreeNode]
///
/// When possible data in this enum should be accessed via [NodeGuard] instead of normal references.
/// Especially parent and child nodes should be accessed via [NodeGuard].
#[derive_where::derive_where(Debug)]
pub(crate) enum MemTreeNode<I> {
    Node {
        /// A pointer to the parent node or none if this is the root.
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
        /// A pointer to the parent node or none if this is the root.
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
    fn new_from(
        device_node: TreeNode,
        parent: Option<NonNull<MemTreeNode<I>>>,
    ) -> Result<Self, AllocError> {
        let result = match device_node {
            TreeNode::Leave { files, .. } => Self::Leave {
                parent,
                files: files
                    .iter()
                    .map(|f| Arc::try_new(f.clone()))
                    .collect::<Result<StaticVec<Arc<FileNode>, LEAVE_MAX_FILE_COUNT>, AllocError>>(
                    )?,
                dirty: false,
                lock: UnsafeTicketLock::new(),
            },
            TreeNode::Node { children, .. } => Self::Node {
                parent,
                children: children
                    .iter()
                    .map(|(device_link, max)| {
                        (
                            MemTreeLink {
                                node: None,
                                device_ptr: Some(*device_link),
                            },
                            *max,
                        )
                    })
                    .collect(),
                dirty: false,
                dirty_children: false,
                lock: UnsafeTicketLock::new(),
            },
        };
        Ok(result)
    }

    fn to_tree_node(&self, parent: Option<DevicePointer<TreeNode>>) -> TreeNode {
        match self {
            MemTreeNode::Node { children, .. } => TreeNode::Node {
                parent,
                children: children
                    .iter()
                    .map(|(link, max_id)| {
                        (
                            link.device_ptr
                                .expect("device_ptr should be set at this point"),
                            *max_id,
                        )
                    })
                    .collect(),
            },
            MemTreeNode::Leave { files, .. } => TreeNode::Leave {
                parent,
                files: files.iter().map(|f| (**f).clone()).collect(),
            },
        }
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

    pub fn dirty_mut(&mut self) -> &mut bool {
        match self {
            MemTreeNode::Node { dirty, .. } => dirty,
            MemTreeNode::Leave { dirty, .. } => dirty,
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
                        (Some(_), None) => {}
                        (None, Some(_)) | (None, None) => {
                            panic!("Only the last child should have a max of None")
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
                        (Some(_), None) => {}
                        (None, Some(_)) | (None, None) => {
                            panic!("Only the last child should have a max of None")
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
    device_ptr: Option<DevicePointer<TreeNode>>,
}

/// This is required for the safety guarantees in [MemTreeLink]
///
/// This is not strictly true. Access to the MemTreeNode is guarded by locks,
/// but this is still usefull, as it prevents us from accidentally keeping
/// pointers around that should no longer be accessed.
impl<I> !Clone for MemTreeLink<I> {}

impl<I> core::fmt::Debug for MemTreeLink<I> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let node = match self.node {
            Some(_) => "Some",
            None => "None",
        };
        f.debug_struct("MemTreeLink")
            .field("node", &node)
            .field("device_ptr", &self.device_ptr)
            .finish()
    }
}

impl<I: InterruptState> MemTreeLink<I> {
    /// Ensures that the link data is loaded into memory.
    #[inline]
    fn resolve<D: BlockDevice>(
        &mut self,
        device: &D,
        parent_node: Option<NonNull<MemTreeNode<I>>>,
    ) -> Result<&mut Self, FsError> {
        if self.node.is_some() {
            return Ok(self);
        }

        let Some(device_ptr) = self.device_ptr else {
            panic!("A node that is not stored in memory, should always be stored on device");
        };

        let device_node = unsafe {
            // Safety: device_ptr should be a valid pointer on the device
            device.read_pointer(device_ptr).map_err(map_device_error)?
        };

        self.node = Some(Box::try_new(MemTreeNode::new_from(
            device_node,
            parent_node,
        )?)?);

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

    /// Flushes any in memory changes to the device
    ///
    /// # Errors
    ///
    /// If this fails with an Error the on device tree should always be in a mostly valid
    /// state.
    /// Only the parent ptr might be invalid.t
    /// TODO that might be a reason not to store it. We don't need it for access anyways.
    ///
    /// However there is no guarantee that all writes have been performed.
    ///
    /// This function only updates the `dirty` flag of self, therefor the in memory
    /// tree should still reflect the latest changes.
    /// It might be possible to attempt to flush changes depending on the device error.
    fn flush_to_device<D: BlockDevice>(
        &mut self,
        parent_ptr: Option<DevicePointer<TreeNode>>,
        device: &mut D,
        block_allocator: &mut BlockAllocator,
        // TODO add ignore dirty flag for error recovery
    ) -> Result<(), FsError> {
        let Some(node) = self.node.as_mut() else {
            assert!(
                self.device_ptr.is_some(),
                "Using a MemTree that was created by calling 'invalid' is prohibited"
            );
            return Ok(());
        };

        let device_ptr = if let Some(device_ptr) = self.device_ptr {
            device_ptr
        } else {
            let device_ptr = block_allocator.allocate_block()?;
            let device_ptr = DevicePointer::new(device_ptr);
            self.device_ptr = Some(device_ptr);
            device_ptr
        };

        let mut node = NodeGuard::<_, node_guard::MutChild>::lock(node);
        let node = node.borrow_mut();

        let dirty = match node {
            MemTreeNode::Node {
                children,
                dirty,
                dirty_children,
                ..
            } => {
                if *dirty_children {
                    for (child_link, _) in children {
                        child_link.flush_to_device(Some(device_ptr), device, block_allocator)?;
                    }
                    *dirty_children = false;
                }
                *dirty
            }
            MemTreeNode::Leave { dirty, .. } => *dirty,
        };
        if dirty {
            trace!(
                "flush mem tree link to device:\n{:#?}\n{:#?}",
                device_ptr, node
            );
            let tree_node = Block::new(node.to_tree_node(parent_ptr));

            device
                .write_block(device_ptr.lba, tree_node.block_data())
                .map_err(map_device_error)?;

            *node.dirty_mut() = false;
        }

        Ok(())
    }
}

struct ChildrenForRebalance<'l, I: InterruptState> {
    left_guard: NodeGuard<'l, I, node_guard::MutChild>,
    left_max_id: &'l mut Option<FileId>,
    right_guard: NodeGuard<'l, I, node_guard::MutChild>,
    right_max_id: &'l mut Option<FileId>,
    mode: DeleteRebalanceMode,
}

enum DeleteRebalanceMode {
    TakeFromRight,
    TakeFromLeft,
    Merge { merge_left_index: usize },
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

    use shared::sync::lockcell::UnsafeTicketLock;
    use staticvec::StaticVec;
    use testing::{
        KernelTestError, TestUnwrapExt, kernel_test, multiprocessor::TestInterruptState, t_assert,
        t_assert_eq, t_assert_matches, tfail,
    };

    use crate::{
        BlockGroup, LBA,
        fs_structs::{
            BlockListHead, DevicePointer, DeviceStringHead, FileId, FileNode, FileType, Perm,
            Timestamp,
        },
        interface::test::TestBlockDevice,
        mem_tree::MemTreeError,
    };

    use super::{MemTree, MemTreeLink, MemTreeNode};

    fn create_empty_tree() -> MemTree<TestInterruptState> {
        MemTree::empty(None)
    }

    fn create_file_node(id: u64) -> Arc<FileNode> {
        Arc::new(FileNode {
            id: FileId::try_new(id).unwrap(),
            parent: None,
            typ: FileType::File,
            permissions: [Perm::all(); 4],
            _unused: [0; 3],
            uid: 0,
            gid: 0,
            size: 0,
            created_at: Timestamp::zero(),
            modified_at: Timestamp::zero(),
            block_data: BlockListHead::Single(BlockGroup::new(
                LBA::new(0).unwrap(),
                LBA::new(0).unwrap(),
            )),
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

    #[kernel_test]
    fn test_delete_100_files() -> Result<(), KernelTestError> {
        let mut tree = create_empty_tree();

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
            tree.delete(&TestBlockDevice, file_id(*id)).tunwrap()?;
        }
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
}
