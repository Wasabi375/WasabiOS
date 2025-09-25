//! [NodeGuard] provides save access to a [super::MemTreeNode].

use alloc::boxed::Box;
use core::{marker::PhantomData, ptr::NonNull};
use static_assertions::const_assert;

use shared::sync::InterruptState;

use crate::{
    fs_structs::{FileId, NODE_MAX_CHILD_COUNT},
    interface::BlockDevice,
};

use super::{DeleteRebalanceMode, MemTreeLink, MemTreeNode};

pub trait Borrow {
    fn is_mut() -> bool;
    fn is_ref() -> bool {
        !Self::is_mut()
    }
    fn drop_check() -> bool {
        Self::is_mut()
    }
}
pub trait BorrowMut: Borrow {}
pub trait IntoParent: BorrowMut {}

/// Marker for Guards that allow for mutable access, this includes
/// upgrading into the parent nodes guard
///
/// Guard must be manually dropped via [super::MemTree::unlock_upwards] or
/// [NodeGuard::awaken_ref]/[NodeGuard::awaken_mut]
pub struct Mut {}
/// Marker for Guards that only allow for readonly access
pub struct Immut {}
/// Marker for Guards that allow for mutable access, but *can't* be upgraded
/// to the parent node's guard.
///
/// Accessing the children is allowed as long as their lock is taken first
pub struct MutChild {}

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
impl IntoParent for Mut {}

impl Borrow for MutChild {
    fn is_mut() -> bool {
        true
    }
    fn drop_check() -> bool {
        false
    }
}
impl BorrowMut for MutChild {}

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
            if B::drop_check() {
                panic!("Mut Node guard must not be dropped. Call awaken or into_parent instead");
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
    /// Creates a new node guard
    ///
    /// # Safety
    ///
    /// the node bust be properly locked, depending on `B`.
    pub unsafe fn new(node: &'a mut MemTreeNode<I>) -> Self {
        Self {
            node: node.into(),
            drop_check: true,
            _lifetime: PhantomData,
            _borrow: PhantomData,
        }
    }

    pub fn lock(node: &'a mut MemTreeNode<I>) -> Self {
        node.get_lock().lock();
        unsafe {
            // Safety: lock just taken
            Self::new(node)
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

impl<'a, I: InterruptState, B: IntoParent> NodeGuard<'a, I, B> {
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
        Some(NodeGuard {
            node: self.borrow().get_parent()?,
            drop_check: true,
            _lifetime: PhantomData,
            _borrow: PhantomData,
        })
    }

    /// Convert this guard into a guard of the parent
    ///
    /// This will unlock the current node.
    ///
    /// Returns a [NodeGuard] for the parent and a ptr to the current node
    /// or `Err(self)` if no parent exists.
    #[allow(clippy::type_complexity)]
    pub fn into_parent(mut self) -> Result<(NodeGuard<'a, I, Mut>, NonNull<MemTreeNode<I>>), Self> {
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
}

impl<'a, I: InterruptState, B: BorrowMut> NodeGuard<'a, I, B> {
    pub fn borrow_mut(&mut self) -> &mut MemTreeNode<I> {
        // Safety: NodeGuard is BorrowMut, therefor we have unique access
        unsafe { self.node.as_mut() }
    }
}

impl<I: InterruptState, B: Borrow> core::fmt::Debug for NodeGuard<'_, I, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NodeGuard")
            .field_with("ptr", |f| f.write_fmt(format_args!("{:#p}", self.node)))
            .field("mutable", &B::is_mut())
            .field("do_drop_check", &B::drop_check())
            .field("drop_check", &self.drop_check)
            .finish()
    }
}
