use core::{
    cell::UnsafeCell,
    fmt::Display,
    hint::spin_loop,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

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
}

#[derive(Debug)]
pub struct LockCellGuard<'l, T, M: ?Sized + LockCellInternal<T>> {
    mutex: &'l M,
    _t: PhantomData<T>,
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
pub struct SpinLock<T> {
    open: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T> Send for SpinLock<T> {}
unsafe impl<T> Sync for SpinLock<T> {}

impl<T> LockCellInternal<T> for SpinLock<T> {
    unsafe fn get(&self) -> &T {
        &*self.data.get()
    }

    unsafe fn get_mut(&self) -> &mut T {
        &mut *self.data.get()
    }

    unsafe fn unlock<'s, 'l: 's>(&'s self, _guard: &mut LockCellGuard<'l, T, Self>) {
        self.open.store(true, Ordering::SeqCst);
    }
}

impl<T> SpinLock<T> {
    pub fn new(data: T) -> Self {
        Self {
            open: AtomicBool::new(true),
            data: UnsafeCell::from(data),
        }
    }
}

impl<T> LockCell<T> for SpinLock<T> {
    fn lock(&self) -> LockCellGuard<'_, T, SpinLock<T>> {
        loop {
            match self
                .open
                .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(_) => spin_loop(),
            }
        }

        LockCellGuard {
            mutex: self,
            _t: PhantomData::default(),
        }
    }
}

impl<T: Default> Default for SpinLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> From<T> for SpinLock<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}
