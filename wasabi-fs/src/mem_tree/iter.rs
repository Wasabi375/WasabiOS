use core::ops::Deref;

use alloc::collections::VecDeque;
use shared::sync::{lockcell::UnsafeTicketLock, InterruptState};

use super::{MemTree, MemTreeNode};

pub(crate) struct TreeNodeGuard<'a, I: InterruptState> {
    node: &'a MemTreeNode<I>,
}

impl<I: InterruptState> Deref for TreeNodeGuard<'_, I> {
    type Target = MemTreeNode<I>;

    fn deref(&self) -> &Self::Target {
        self.node
    }
}

impl<I: InterruptState> Drop for TreeNodeGuard<'_, I> {
    fn drop(&mut self) {
        unsafe {
            // Safety: locked on guard creation
            self.node.get_lock().unlock()
        }
    }
}

// FIXME race-condition: we should lock the last node until next is called.
//  This also allows me to return a ref instead of a custom guard.
//  Also requires a drop impl (to drop last lock) and special handling for root_lock
pub(crate) struct MemTreeNodeIter<'a, I: InterruptState> {
    tree: &'a MemTree<I>,
    open: VecDeque<&'a MemTreeNode<I>>,
}

impl<'a, I: InterruptState> MemTreeNodeIter<'a, I> {
    pub(super) fn new(tree: &'a MemTree<I>) -> Self {
        tree.root_lock.lock();
        let root = unsafe {
            // Safety: we have the root lock
            tree.get_root().as_ref()
        };
        unsafe {
            // Safety: just locked above
            tree.root_lock.unlock();
        }
        Self {
            tree,
            open: root.iter().map(|it| *it).collect(),
        }
    }
}

impl<'a, I: InterruptState> Iterator for MemTreeNodeIter<'a, I> {
    type Item = TreeNodeGuard<'a, I>;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(current) = self.open.pop_front() else {
            return None;
        };

        current.get_lock().lock();

        if let MemTreeNode::Node { children, .. } = current {
            self.open.extend(children.iter().filter_map(|node| unsafe {
                // Safety: we locked the parent above
                node.0.as_ref()
            }));
        }

        Some(TreeNodeGuard { node: current })
    }
}
