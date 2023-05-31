use crate::types::CoreId;
use core::{
    cell::UnsafeCell,
    fmt::Display,
    hint::spin_loop,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

/// Trait that allows access to OS-level constructs defining interrupt state,
/// exception state, unique core IDs, and enter/exit lock (for interrupt
/// disabling and enabling) primitives.
pub trait InterruptState {
    /// Returns `true` if we're currently in an interrupt
    fn in_interrupt() -> bool;

    /// Returns `true` if we're currently in an exception. Which indicates that
    /// a lock cannot be held as we may have pre-empted a non-preemptable lock
    fn in_exception() -> bool;

    /// Gets the ID of the running core. It's required that this core ID is
    /// unique to the core.
    fn core_id() -> CoreId;

    /// A lock which does not allow interrupting was taken, and thus interrupts
    /// must be disabled. It's up to the callee to handle the nesting of the
    /// interrupt status. Eg. using a refcount of number of interrupt disable
    /// requests
    fn enter_lock();

    /// A lock which does not allow interrupting was released, and thus
    /// interrupts can be enabled. It's up to the callee to handle the nesting
    /// of the interrupt status. Eg. using a refcount of number of interrupt
    /// disable requests
    fn exit_lock();
}

#[doc(hidden)]
pub trait LockCellInternal<T> {
    /// Returns a reference to the data behind the mutex
    ///
    /// # Safety:
    ///
    /// this thread needs to hold the lock
    unsafe fn get(&self) -> &T;
    /// Returns a mutable reference to the data behind the mutex
    ///
    /// # Safety:
    ///
    /// this thread needs to hold the lock
    unsafe fn get_mut(&self) -> &mut T;

    /// unlocks the mutex
    ///
    /// # Safety:
    ///
    /// this should only be called when the [LockCellGuard] corresponding to this
    /// LockCell is droped.
    unsafe fn unlock<'s, 'l: 's>(&'s self, guard: &mut LockCellGuard<'l, T, Self>);

    /// forces the mutex open, without needing access to the guard
    ///
    /// # Safety:
    ///
    /// the caller ensures that there is no active guard
    unsafe fn force_unlock(&self);
}

#[derive(Debug)]
pub struct LockCellGuard<'l, T, M: ?Sized + LockCellInternal<T>> {
    mutex: &'l M,
    _t: PhantomData<T>,
}

impl<'l, T, M: ?Sized + LockCellInternal<T>> LockCellGuard<'l, T, M> {
    pub fn new(mutex: &'l M) -> Self {
        LockCellGuard {
            mutex,
            _t: PhantomData::default(),
        }
    }
}

impl<T, M: ?Sized + LockCellInternal<T>> !Sync for LockCellGuard<'_, T, M> {}

impl<T, M: ?Sized + LockCellInternal<T>> Deref for LockCellGuard<'_, T, M> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // Safety: There can always be only 1 guard for a given mutex so this is safe
        unsafe { self.mutex.get() }
    }
}

impl<T, M: ?Sized + LockCellInternal<T>> DerefMut for LockCellGuard<'_, T, M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: There can always be only 1 guard for a given mutex so this is safe
        unsafe { self.mutex.get_mut() }
    }
}

impl<T: Display, M: ?Sized + LockCellInternal<T>> Display for LockCellGuard<'_, T, M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (**self).fmt(f)
    }
}

impl<T, M: ?Sized + LockCellInternal<T>> Drop for LockCellGuard<'_, T, M> {
    fn drop(&mut self) {
        unsafe { self.mutex.unlock(self) }
    }
}

pub trait LockCell<T>
where
    Self: LockCellInternal<T>,
{
    fn lock(&self) -> LockCellGuard<'_, T, Self>;
}

#[derive(Debug)]
pub struct SpinLock<T, I> {
    open: AtomicBool,
    data: UnsafeCell<T>,
    holder: UnsafeCell<Option<CoreId>>,
    _interrupt_state: PhantomData<I>,
}

unsafe impl<T, I: InterruptState> Send for SpinLock<T, I> {}
unsafe impl<T, I: InterruptState> Sync for SpinLock<T, I> {}

impl<T, I: InterruptState> LockCellInternal<T> for SpinLock<T, I> {
    unsafe fn get(&self) -> &T {
        &*self.data.get()
    }

    unsafe fn get_mut(&self) -> &mut T {
        &mut *self.data.get()
    }

    unsafe fn unlock<'s, 'l: 's>(&'s self, _guard: &mut LockCellGuard<'l, T, Self>) {
        self.open.store(true, Ordering::SeqCst);
    }

    unsafe fn force_unlock(&self) {
        self.open.store(true, Ordering::SeqCst);
    }
}

impl<T, I> SpinLock<T, I> {
    pub fn new(data: T) -> Self {
        Self {
            open: AtomicBool::new(true),
            data: UnsafeCell::new(data),
            holder: UnsafeCell::new(None),
            _interrupt_state: PhantomData::default(),
        }
    }
}

impl<T, I: InterruptState> LockCell<T> for SpinLock<T, I> {
    #[track_caller]
    fn lock(&self) -> LockCellGuard<'_, T, Self> {
        assert!(
            !I::in_interrupt(),
            "SpinLock can not be preemted. Use other lock type instead. TODO impl ticket lock"
        );
        loop {
            match self
                .open
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(_) => {
                    let holder = unsafe { *self.holder.get() };
                    if let Some(holder) = holder && holder == I::core_id() {
                        panic!("Deadlock detected!");
                    }
                    spin_loop()
                }
            }
        }

        unsafe {
            *self.holder.get() = Some(I::core_id());
        }

        LockCellGuard {
            mutex: self,
            _t: PhantomData::default(),
        }
    }
}

impl<T: Default, I> Default for SpinLock<T, I> {
    fn default() -> Self {
        Self::new(T::default())
    }
}
