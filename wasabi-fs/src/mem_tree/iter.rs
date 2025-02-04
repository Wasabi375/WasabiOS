use core::ops::Deref;

use alloc::collections::VecDeque;
use shared::sync::InterruptState;

use super::{MemTree, MemTreeNode};

pub(crate) struct MemTreeNodeIter<'a, I: InterruptState> {
    tree: &'a MemTree<I>,
    open: VecDeque<&'a MemTreeNode<I>>,
}

impl<'a, I: InterruptState> MemTreeNodeIter<'a, I> {
    pub(super) fn new(tree: &'a mut MemTree<I>) -> Self {
        let root = unsafe {
            // Safety: we ignore the lock as we have unique access
            tree.get_root().as_ref()
        };
        Self {
            tree,
            open: root.iter().map(|it| *it).collect(),
        }
    }
}

impl<'a, I: InterruptState> Iterator for MemTreeNodeIter<'a, I> {
    type Item = &'a MemTreeNode<I>;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(current) = self.open.pop_front() else {
            return None;
        };

        if let MemTreeNode::Node { children, .. } = current {
            self.open.extend(children.iter().filter_map(|node| unsafe {
                // Safety: we ignore the lock as we have unique access
                node.0.as_ref()
            }));
        }

        Some(current)
    }
}
