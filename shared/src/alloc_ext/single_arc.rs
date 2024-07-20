use core::{
    fmt::{Display, Pointer},
    hash::Hash,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    sync::atomic::{self, AtomicBool, AtomicUsize, Ordering},
};

use alloc::boxed::Box;

/// An Arc-like data structure that allows for mutable access
///
/// There can always be only 1 Strong reference, which has mutable access
/// and multiple Weak references that can only check the number of references.
#[repr(transparent)]
pub struct SingleArc<T: ?Sized> {
    inner: NonNull<Inner<T>>,
    phantom: PhantomData<Inner<T>>,
}

/// A weak reference to a [SingleArc]
#[repr(transparent)]
#[derive(Clone)]
pub struct WeakSingleArc<T: ?Sized> {
    inner: NonNull<Inner<T>>,
    phantom: PhantomData<Inner<T>>,
}

#[repr(C)]
struct Inner<T: ?Sized> {
    strong_ref: AtomicBool,
    ref_count: AtomicUsize,

    data: T,
}

impl<T: ?Sized> Drop for SingleArc<T> {
    fn drop(&mut self) {
        self.inner().strong_ref.store(false, Ordering::SeqCst);
        if self.inner().ref_count.fetch_sub(1, Ordering::SeqCst) == 1 {
            let _ = unsafe {
                // This fence is needed to prevent reordering of the use and deletion
                // of the data.
                atomic::fence(Ordering::Acquire);
                // Safety: ptr was created using Box::leak and we have the last reference
                Box::from_raw(self.inner.as_ptr())
            };
        }
    }
}

impl<T: ?Sized> Drop for WeakSingleArc<T> {
    fn drop(&mut self) {
        if self.atomic_ref_count().fetch_sub(1, Ordering::SeqCst) == 1 {
            let _ = unsafe {
                // This fence is needed to prevent reordering of the use and deletion
                // of the data.
                atomic::fence(Ordering::Acquire);
                // Safety: ptr was created using Box::leak and we have the last reference
                Box::from_raw(self.inner.as_ptr())
            };
        }
    }
}

unsafe impl<T: ?Sized + Sync + Send> Send for SingleArc<T> {}
unsafe impl<T: ?Sized + Sync + Send> Sync for SingleArc<T> {}

impl<T> SingleArc<T> {
    pub fn new(value: T) -> Self {
        let inner = Box::new(Inner {
            data: value,
            strong_ref: AtomicBool::new(true),
            ref_count: AtomicUsize::new(1),
        });

        let inner_ptr = Box::leak(inner);

        SingleArc {
            inner: NonNull::from(inner_ptr),
            phantom: PhantomData,
        }
    }
}

impl<T: ?Sized> SingleArc<T> {
    fn inner_mut(&mut self) -> &mut Inner<T> {
        // Safety: only self has access to data directly
        unsafe { self.inner.as_mut() }
    }

    fn inner(&self) -> &Inner<T> {
        // Safety: only self has access to data directly
        unsafe { self.inner.as_ref() }
    }

    pub fn downgrade(&self) -> WeakSingleArc<T> {
        let inner = self.inner();
        inner.ref_count.fetch_add(1, Ordering::SeqCst);
        WeakSingleArc {
            inner: self.inner.clone(),
            phantom: PhantomData,
        }
    }
}

impl<T: ?Sized> AsRef<T> for SingleArc<T> {
    fn as_ref(&self) -> &T {
        &self.inner().data
    }
}

impl<T: ?Sized> AsMut<T> for SingleArc<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.inner_mut().data
    }
}

impl<T: ?Sized> Deref for SingleArc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T: ?Sized> DerefMut for SingleArc<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

impl<T: ?Sized + core::fmt::Debug> core::fmt::Debug for SingleArc<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl<T: ?Sized + Display> Display for SingleArc<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl<T: ?Sized + Eq> Eq for SingleArc<T> {}

impl<T: ?Sized + PartialEq> PartialEq for SingleArc<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl<T: ?Sized + PartialEq> PartialEq<T> for SingleArc<T> {
    fn eq(&self, other: &T) -> bool {
        self.as_ref() == other
    }
}

impl<T: ?Sized + Ord> Ord for SingleArc<T> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_ref().cmp(other.as_ref())
    }
}

impl<T: ?Sized + PartialOrd> PartialOrd for SingleArc<T> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        self.as_ref().partial_cmp(other.as_ref())
    }
}

impl<T: ?Sized + Hash> Hash for SingleArc<T> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state)
    }
}

impl<T: ?Sized> Pointer for SingleArc<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.inner.fmt(f)
    }
}

impl<T: ?Sized> Unpin for SingleArc<T> {}

impl<T: ?Sized> WeakSingleArc<T> {
    fn atomic_strong_ref(&self) -> &AtomicBool {
        // Safetey: data is a valid ptr, since we still have a weak ref to it.
        // FIXME: am I allowed to call as_ref? This violates aliasing for data
        unsafe { &self.inner.as_ref().strong_ref }
    }

    fn atomic_ref_count(&self) -> &AtomicUsize {
        // Safetey: data is a valid ptr, since we still have a weak ref to it.
        // FIXME: am I allowed to call as_ref? This violates aliasing for data
        unsafe { &self.inner.as_ref().ref_count }
    }

    /// Try to upgrade to a [SingleArc].
    pub fn try_upgrade(&self) -> Option<SingleArc<T>> {
        match self.atomic_strong_ref().compare_exchange(
            false,
            true,
            Ordering::SeqCst,
            Ordering::SeqCst,
        ) {
            Ok(_) => {
                self.atomic_ref_count().fetch_add(1, Ordering::SeqCst);
                Some(SingleArc {
                    inner: self.inner,
                    phantom: PhantomData,
                })
            }
            Err(_) => None,
        }
    }

    /// The number of references to this [SingleArc].
    ///
    /// This inclueds all [WeakSingleArc]s and the potential single [SingleArc]
    pub fn ref_count(&self) -> usize {
        self.atomic_ref_count().load(Ordering::Acquire)
    }

    /// `true` if a [SingleArc] exists for this [WeakSingleArc]
    pub fn has_strong_ref(&self) -> bool {
        self.atomic_strong_ref().load(Ordering::Acquire)
    }
}

/// A [SingleArc] that always has at most 1 corresponding [Weak] pointer.
pub struct Strong<T: ?Sized>(SingleArc<T>);

impl<T> Strong<T> {
    /// Creates a new [Strong], [Weak] pair
    pub fn new(value: T) -> (Strong<T>, Weak<T>) {
        let single = SingleArc::new(value);
        let weak = single.downgrade();
        (Strong(single), Weak(weak))
    }
}

impl<T: ?Sized> AsRef<T> for Strong<T> {
    fn as_ref(&self) -> &T {
        self.0.as_ref()
    }
}

impl<T: ?Sized> AsMut<T> for Strong<T> {
    fn as_mut(&mut self) -> &mut T {
        self.0.as_mut()
    }
}

impl<T: ?Sized> Deref for Strong<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl<T: ?Sized> DerefMut for Strong<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut()
    }
}

impl<T: ?Sized + core::fmt::Debug> core::fmt::Debug for Strong<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.as_ref().fmt(f)
    }
}

impl<T: ?Sized + Display> Display for Strong<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.as_ref().fmt(f)
    }
}

impl<T: ?Sized + Eq> Eq for Strong<T> {}

impl<T: ?Sized + PartialEq> PartialEq for Strong<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_ref() == other.as_ref()
    }
}

impl<T: ?Sized + PartialEq> PartialEq<T> for Strong<T> {
    fn eq(&self, other: &T) -> bool {
        self.0.as_ref() == other
    }
}

impl<T: ?Sized + Ord> Ord for Strong<T> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.0.as_ref().cmp(other.as_ref())
    }
}

impl<T: ?Sized + PartialOrd> PartialOrd for Strong<T> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        self.0.as_ref().partial_cmp(other.as_ref())
    }
}

impl<T: ?Sized + Hash> Hash for Strong<T> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.as_ref().hash(state)
    }
}

impl<T: ?Sized> Pointer for Strong<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

/// The single [Weak] pointer for a [Strong].
pub struct Weak<T: ?Sized>(WeakSingleArc<T>);

impl<T> Weak<T> {
    /// Creates a new [Weak] that can later be upgraded to a [Strong]
    pub fn new(value: T) -> Self {
        let single = SingleArc::new(value);
        let weak = single.downgrade();
        Self(weak)
    }
}

impl<T: ?Sized> Weak<T> {
    /// Try to upgrade to a [Strong].
    pub fn try_upgrade(&self) -> Option<Strong<T>> {
        self.0.try_upgrade().map(|inner| Strong(inner))
    }

    /// `true` if a [Strong] exists for this [Weak]
    pub fn has_strong_ref(&self) -> bool {
        self.0.has_strong_ref()
    }
}
