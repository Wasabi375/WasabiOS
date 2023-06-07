//! LockCell implementations
//!
//! [TicketLock] is a [LockCell] implementation, that can be both preemtable or
//! not depending on how  it is created ([`TicketLock::new_preemtable`] vs [`TicketLock::new`]).
//!
//! [UnwrapLock] is a [LockCell] wrapper that allows accessing a `UnwrapLock<MaybeUninit<T>>` as if it
//! is an `LockCell<T>`.
use crate::types::CoreId;
use core::{
    cell::UnsafeCell,
    fmt::Display,
    hint::spin_loop,
    marker::PhantomData,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU16, AtomicU64, Ordering},
};
use paste::paste;

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

/// unsafe internals used by [LockCell]s and [LockCellGuard].
///
/// Normally this shouldn't be used unless you implement a [LockCell].
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

/// A guard structure that is used to guard a lock.
///
/// This allows safe access to the value inside of a [LockCell].
/// When this is dropped, the [LockCell] is unlocked again.
///
/// This can be obtained from [`LockCell::lock`]
#[derive(Debug)]
pub struct LockCellGuard<'l, T, M: ?Sized + LockCellInternal<T>> {
    /// the lockcell that is guarded by `self`
    lockcell: &'l M,
    /// phantom data for the type `T`
    _t: PhantomData<T>,
}

impl<'l, T, M: ?Sized + LockCellInternal<T>> LockCellGuard<'l, T, M> {
    /// creates a new guard. This should only be called if you implement a [LockCell].
    ///
    /// # Safety:
    ///
    /// The caller must ensure that only 1 LockClellGuard exists for any given
    /// `lockcell` at a time.
    pub unsafe fn new(lockcell: &'l M) -> Self {
        LockCellGuard {
            lockcell,
            _t: PhantomData::default(),
        }
    }
}

impl<T, M: ?Sized + LockCellInternal<T>> !Sync for LockCellGuard<'_, T, M> {}

impl<T, M: ?Sized + LockCellInternal<T>> Deref for LockCellGuard<'_, T, M> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // Safety: There can always be only 1 guard for a given mutex so this is safe
        unsafe { self.lockcell.get() }
    }
}

impl<T, M: ?Sized + LockCellInternal<T>> DerefMut for LockCellGuard<'_, T, M> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: There can always be only 1 guard for a given mutex so this is safe
        unsafe { self.lockcell.get_mut() }
    }
}

impl<T: Display, M: ?Sized + LockCellInternal<T>> Display for LockCellGuard<'_, T, M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (**self).fmt(f)
    }
}

impl<T, M: ?Sized + LockCellInternal<T>> Drop for LockCellGuard<'_, T, M> {
    fn drop(&mut self) {
        unsafe { self.lockcell.unlock(self) }
    }
}

/// A trait representing a lock cell that guards simultaneus access to a value.
pub trait LockCell<T>
where
    Self: LockCellInternal<T> + Send + Sync,
{
    /// gives out access to the value of this lock. Blocks until access is granted.
    fn lock(&self) -> LockCellGuard<'_, T, Self>;
}

/// A ticket lock implementation for [LockCell]
///
/// [TicketLock]s are preemtable and can be used within interrupts.
#[derive(Debug)]
pub struct TicketLock<T, I> {
    /// the current ticket that can access the lock
    current_ticket: AtomicU64,
    /// the next ticket we give out
    next_ticket: AtomicU64,
    /// the data within the lock
    data: UnsafeCell<T>,
    /// the current core holding the lock
    owner: AtomicU16,
    /// set if the lock is usable in interrupts
    preemtable: bool,
    /// phantom access to the cores interrupt state
    _interrupt_state: PhantomData<I>,
}

unsafe impl<T, I: InterruptState> Send for TicketLock<T, I> {}
unsafe impl<T, I: InterruptState> Sync for TicketLock<T, I> {}

impl<T, I> TicketLock<T, I> {
    /// creates a new [TicketLock]
    pub const fn new(data: T) -> Self {
        Self {
            current_ticket: AtomicU64::new(0),
            next_ticket: AtomicU64::new(0),
            data: UnsafeCell::new(data),
            owner: AtomicU16::new(!0),
            preemtable: false,
            _interrupt_state: PhantomData,
        }
    }

    /// creates a new preemtable [TicketLock]
    pub const fn new_preemtable(data: T) -> Self {
        Self {
            current_ticket: AtomicU64::new(0),
            next_ticket: AtomicU64::new(0),
            data: UnsafeCell::new(data),
            owner: AtomicU16::new(!0),
            preemtable: true,
            _interrupt_state: PhantomData,
        }
    }
}

impl<T, I: InterruptState> LockCell<T> for TicketLock<T, I> {
    #[track_caller]
    fn lock(&self) -> LockCellGuard<'_, T, Self> {
        assert!(
            self.preemtable || !I::in_interrupt(),
            "use of non-preemtable lock in interrupt"
        );

        if self.preemtable {
            I::enter_lock();
        }

        let ticket = self.next_ticket.fetch_add(1, Ordering::SeqCst);

        while self.current_ticket.load(Ordering::SeqCst) != ticket {
            let owner = self.owner.load(Ordering::Acquire);
            if owner != !0 && owner == I::core_id().0 as u16 {
                panic!("Deadlock detected");
            }
            spin_loop();
        }

        self.owner.store(I::core_id().0 as u16, Ordering::Release);

        LockCellGuard {
            lockcell: self,
            _t: PhantomData,
        }
    }
}

impl<T, I: InterruptState> LockCellInternal<T> for TicketLock<T, I> {
    unsafe fn get(&self) -> &T {
        &*self.data.get()
    }

    unsafe fn get_mut(&self) -> &mut T {
        &mut *self.data.get()
    }

    unsafe fn unlock<'s, 'l: 's>(&'s self, _guard: &mut LockCellGuard<'l, T, Self>) {
        self.owner.store(!0, Ordering::Release);
        self.current_ticket.fetch_add(1, Ordering::SeqCst);
        if self.preemtable {
            I::exit_lock();
        }
    }

    unsafe fn force_unlock(&self) {
        self.owner.store(!0, Ordering::Release);
        self.current_ticket.fetch_add(1, Ordering::SeqCst);
        if self.preemtable {
            I::exit_lock();
        }
    }
}

impl<T: Default, I: InterruptState> Default for TicketLock<T, I> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

/// A wrapper for a [`LockCell`] of an `MaybeUninit<T>`.
///
/// unlike a normal [LockCell], [`UnwrapLock::lock`] will return `T`
/// or panic if the value was not initialized
pub struct UnwrapLock<T, L: LockCell<MaybeUninit<T>>> {
    /// inner lockcell that holds the `MaybeUninit<T>`
    lockcell: L,
    _t: PhantomData<T>,
}

macro_rules! unwrapLockWrapper {
    (
        $(#[$outer:meta])*
        $lock_type:ident
    ) => {

        paste! {
            $(#[$outer])*
            pub type [<Unwrap $lock_type>]<T, I> = UnwrapLock<T, $lock_type<MaybeUninit<T>, I>>;

            impl<T, I: InterruptState> [<Unwrap $lock_type>]<T, I> {
                /// creates a new [Self] that is uninitialized
                pub const unsafe fn new_uninit() -> Self {
                    UnwrapLock::new($lock_type::new(MaybeUninit::uninit()))
                }

                /// creates a new preemtable [Self] that is uninitialized
                pub const unsafe fn new_preemtable_uninit() -> Self {
                    UnwrapLock::new($lock_type::new_preemtable(MaybeUninit::uninit()))
                }
            }
        }
    };
}

unwrapLockWrapper! {
    /// A [UnwrapLock] wrapper for [TicketLock]
    TicketLock
}

impl<T, L: LockCell<MaybeUninit<T>>> Drop for UnwrapLock<T, L> {
    fn drop(&mut self) {
        unsafe {
            self.lock_uninit().assume_init_drop();
        }
    }
}

impl<T, L: LockCell<MaybeUninit<T>> + Default> Default for UnwrapLock<T, L> {
    fn default() -> Self {
        Self {
            lockcell: Default::default(),
            _t: Default::default(),
        }
    }
}

unsafe impl<T, L: LockCell<MaybeUninit<T>>> Send for UnwrapLock<T, L> {}
unsafe impl<T, L: LockCell<MaybeUninit<T>>> Sync for UnwrapLock<T, L> {}

impl<T, L: LockCell<MaybeUninit<T>>> UnwrapLock<T, L> {
    /// creates a new [UnwrapLock] from the given `inner` [LockCell]
    ///
    /// # Safety:
    ///
    /// the caller ensures that the lock is initialized with a value, before
    /// [UnwrapLock::lock] is called.
    pub const unsafe fn new(inner: L) -> Self {
        Self {
            lockcell: inner,
            _t: PhantomData,
        }
    }

    /// gives access to the locked `MaybeUninit`. Blocks until the lock is accessible.
    ///
    /// This is intented for initialization of the [UnwrapLock]
    pub fn lock_uninit(&self) -> LockCellGuard<'_, MaybeUninit<T>, Self> {
        let inner_guard = self.lockcell.lock();
        core::mem::forget(inner_guard);
        unsafe { LockCellGuard::new(self) }
    }
}

impl<T, L: LockCell<MaybeUninit<T>>> LockCell<T> for UnwrapLock<T, L> {
    fn lock(&self) -> LockCellGuard<'_, T, Self> {
        let inner_guard = self.lockcell.lock();
        core::mem::forget(inner_guard);
        unsafe { LockCellGuard::new(self) }
    }
}

impl<T, L: LockCell<MaybeUninit<T>>> LockCellInternal<T> for UnwrapLock<T, L> {
    unsafe fn get(&self) -> &T {
        self.lockcell.get().assume_init_ref()
    }

    unsafe fn get_mut(&self) -> &mut T {
        self.lockcell.get_mut().assume_init_mut()
    }

    unsafe fn unlock<'s, 'l: 's>(&'s self, _guard: &mut LockCellGuard<'l, T, Self>) {
        self.lockcell.force_unlock();
    }

    unsafe fn force_unlock(&self) {
        self.lockcell.force_unlock();
    }
}

impl<T, L: LockCell<MaybeUninit<T>>> LockCellInternal<MaybeUninit<T>> for UnwrapLock<T, L> {
    unsafe fn get(&self) -> &MaybeUninit<T> {
        self.lockcell.get()
    }

    unsafe fn get_mut(&self) -> &mut MaybeUninit<T> {
        self.lockcell.get_mut()
    }

    unsafe fn unlock<'s, 'l: 's>(&'s self, _guard: &mut LockCellGuard<'l, MaybeUninit<T>, Self>) {
        self.lockcell.force_unlock();
    }

    unsafe fn force_unlock(&self) {
        self.lockcell.force_unlock();
    }
}
