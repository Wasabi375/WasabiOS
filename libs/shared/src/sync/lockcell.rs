//! LockCell implementations
//!
//! [TicketLock] is a [LockCell] implementation, that can be both preemtable or
//! not depending on how it is created ([`TicketLock::new`] vs
//! [`TicketLock::new_non_preemtable`]).
//!
//! [UnwrapLock] is a [LockCell] wrapper that allows accessing a
//! `UnwrapLock<MaybeUninit<T>>` as if it is an `LockCell<T>`.

#![warn(missing_docs, rustdoc::missing_crate_level_docs)]

use crate::sync::InterruptState;
use core::{
    cell::UnsafeCell,
    fmt::Display,
    hint::spin_loop,
    marker::PhantomData,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, AtomicI64, AtomicU16, AtomicU64, Ordering},
};
use paste::paste;

/// A trait representing a lock cell that guards simultaneus access to a value.
pub trait LockCell<T>
where
    Self: LockCellInternal<T> + Send + Sync,
{
    /// gives out access to the value of this lock. Blocks until access is granted.
    fn lock(&self) -> LockCellGuard<'_, T, Self>;

    /// shater the lock and get a mut ptr to the underlying data
    ///
    /// This may make the lock inoperable
    fn shatter(&self) -> *mut T {
        // TODO what is the difference to sahtter_permanent?
        unsafe { self.get_mut() as *mut T }
    }
}

/// A trait representing a read write lock, that allows for either simultaneous
/// read access to a value or a single write access.
pub trait RWLockCell<T>
where
    Self: LockCell<T> + LockCellInternal<T> + RWCellInternal<T> + Send + Sync,
{
    /// gives out read access to the value of this lock. Block until access is granted.
    fn read(&self) -> ReadCellGuard<'_, T, Self>;

    /// gives out mutable access to the value of this lock. Blocks until access is granted.
    fn write(&self) -> LockCellGuard<'_, T, Self> {
        self.lock()
    }
}

/// unsafe internals used by [LockCell]s and [LockCellGuard].
///
/// Normally this shouldn't be used unless you implement a [LockCell].
pub trait LockCellInternal<T> {
    /// Returns a reference to the data behind the mutex
    ///
    /// # Safety
    ///
    /// this thread needs to hold the lock
    unsafe fn get(&self) -> &T;
    /// Returns a mutable reference to the data behind the mutex
    ///
    /// # Safety
    ///
    /// this thread needs to hold the lock
    #[allow(clippy::mut_from_ref)]
    unsafe fn get_mut(&self) -> &mut T;

    /// unlocks the mutex
    ///
    /// # Safety
    ///
    /// this should only be called when the [LockCellGuard] corresponding to
    /// this [LockCell] is droped.
    unsafe fn unlock<'s, 'l: 's>(&'s self, guard: &mut LockCellGuard<'l, T, Self>);

    /// forces the mutex open, without needing access to the guard.
    ///
    /// This will reset the interrupt state, just like dropping [LockCellGuard] would.
    ///
    /// # Safety
    ///
    /// the caller ensures that there the guard from [LockCell::lock] is inaccessible.
    /// It is not save to call this, if there is no corresponding `lock` call.
    unsafe fn unlock_no_guard(&self);

    /// "Permanently" shatters the lock. Following calls to [LockCell::lock] will
    /// succeed instantly.
    ///
    /// Some implementations might provide a way to reset the lock, or even reset themself
    /// automatically.
    ///
    /// # Safety
    ///
    /// the caller ensures that there are no guards.
    unsafe fn shatter_permanent(&self);

    /// returns `true` if the LockCell is currently unlocked.
    ///
    /// However the caller can't relly on this fact, since some other
    /// core/interrupt etc could take the lock during or right after this call
    /// finishes.
    fn is_unlocked(&self) -> bool;

    /// returns `true` if the lock is preemtable.
    ///
    /// In that case the lock is useable within interrupts, but must disable
    /// additional interrupts while being held.
    fn is_preemtable(&self) -> bool;
}

/// How any Lock was acquired
#[derive(Debug, Clone, Copy)]
pub enum LockReason {
    /// The lock was properly acquired
    Acquired,
    /// The lock is shattered
    Shattered,
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
    /// How the lock was acquired
    pub reason: LockReason,

    /// phantom data for the type `T`
    _t: PhantomData<T>,
}

impl<'l, T, M: ?Sized + LockCellInternal<T>> LockCellGuard<'l, T, M> {
    /// creates a new guard. This should only be called if you implement a [LockCell].
    ///
    /// # Safety
    ///
    /// The caller must ensure that only 1 LockClellGuard exists for any given
    /// `lockcell` at a time.
    pub unsafe fn new(lockcell: &'l M, reason: LockReason) -> Self {
        LockCellGuard {
            lockcell,
            reason,
            _t: PhantomData,
        }
    }

    /// allows to execute a simple snippet of code in a expression chain.
    /// Mostly used for debug puropses.
    ///
    /// # Example usage
    /// ```no_run
    /// # use log::info;
    /// # use shared::sync::lockcell::LockCell;
    /// # fn test(lock: impl LockCell<u64>) {
    /// lock.lock().also(|_| { info!("lock acuired"); } ).as_ref().clone();
    /// # }
    /// ```
    pub fn also<F: FnOnce(&mut Self)>(mut self, f: F) -> Self {
        f(&mut self);
        self
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

impl<T, M: ?Sized + LockCellInternal<T>> AsRef<T> for LockCellGuard<'_, T, M> {
    fn as_ref(&self) -> &T {
        // Safety: There can always be only 1 guard for a given mutex so this is safe
        unsafe { self.lockcell.get() }
    }
}

impl<T, M: ?Sized + LockCellInternal<T>> AsMut<T> for LockCellGuard<'_, T, M> {
    fn as_mut(&mut self) -> &mut T {
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

/// unsafe internals used by [RWLockCell] and [ReadCellGuard].
///
/// Normally this shouldn't be used unless you implement [RWLockCell]
pub trait RWCellInternal<T>: LockCellInternal<T> {
    /// releases a read guard of the lock.
    /// If there are no read guards left, the lock is unlocked.
    ///
    /// # Safety
    ///
    /// this should only be called when the [ReadCellGuard] corresponding to
    /// this [RWLockCell] is dropped.
    unsafe fn release_read<'s, 'l: 's>(&'s self, guard: &mut ReadCellGuard<'l, T, Self>);

    /// release a [ReadCellGuard] without access to the actual guard.
    /// This is used to implement locks based on other locks, requiring their guards
    /// to return a guard alias. However it is not possible to store the internal
    /// locks guard, so we need a way to release anyways.
    ///
    /// It is valid to implement this using a panic.
    ///
    /// # Safety
    ///
    /// the caller ensures that the simulated guard is no longer accessible.
    /// the caller also ensures that this function is only used on implementations
    /// that support this.
    unsafe fn force_release_read(&self) {}

    /// returns `true` if the [RWLockCell] is currently lockable by reads, meaning
    /// that either noone has a lock or a read has a lock.
    ///
    /// However the caller can't relly on this fact, since some other
    /// core/interrupt etc could take the lock during or right after this call
    /// finishes.
    fn open_to_read(&self) -> bool;
}

/// A guard structure that is used to guard read access to a lock.
///
/// This allows safe "read" access to the value inside of a [RWLockCell].
/// When this is dropped, the [RWLockCell] will unlock again, if there are no other
/// [ReadCellGuard]s for the lock.
///
/// This can be obtained from [`RWLockCell::read`]
#[derive(Debug)]
pub struct ReadCellGuard<'l, T, M: ?Sized + RWCellInternal<T>> {
    rw_cell: &'l M,
    _t: PhantomData<T>,
}

impl<'l, T, M: ?Sized + RWCellInternal<T>> ReadCellGuard<'l, T, M> {
    /// creates a new guard. This should only be called if you implement a [RWLockCell].
    ///
    /// # Safety
    ///
    /// The caller must ensure that only 1 [LockCellGuard] exists for any given
    /// `rw_cell` at a time or multiple [ReadCellGuard]s
    pub unsafe fn new(rw_cell: &'l M) -> Self {
        ReadCellGuard {
            rw_cell,
            _t: PhantomData,
        }
    }
}

impl<T, M: ?Sized + RWCellInternal<T>> !Sync for ReadCellGuard<'_, T, M> {}

impl<T, M: ?Sized + RWCellInternal<T>> Deref for ReadCellGuard<'_, T, M> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // Safety: While the guard exists there can't be any mut access to the lock
        // and we only give out immutable access
        unsafe { self.rw_cell.get() }
    }
}

impl<T: Display, M: ?Sized + RWCellInternal<T>> Display for ReadCellGuard<'_, T, M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (**self).fmt(f)
    }
}

impl<T, M: ?Sized + RWCellInternal<T>> Drop for ReadCellGuard<'_, T, M> {
    fn drop(&mut self) {
        unsafe {
            self.rw_cell.release_read(self);
        }
    }
}

/// A Ticket lock that does not hold data or gives out guards.
///
/// It is the job of the user of this struct to properly lock and unlock
/// and to manage data associated with the lock
#[derive_where::derive_where(Debug)]
pub struct UnsafeTicketLock<I> {
    /// the current ticket that can access the lock
    current_ticket: AtomicU64,
    /// the next ticket we give out
    next_ticket: AtomicU64,
    /// true if the lock is shattered
    shattered: AtomicBool,
    /// the current core holding the lock
    owner: AtomicU16,
    /// set if the lock is *not* usable in interrupts
    pub preemtable: bool,
    /// phantom access to the cores interrupt state
    _interrupt_state: PhantomData<I>,
}

unsafe impl<I: InterruptState> Send for UnsafeTicketLock<I> {}
unsafe impl<I: InterruptState> Sync for UnsafeTicketLock<I> {}

impl<I> Default for UnsafeTicketLock<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I> UnsafeTicketLock<I> {
    /// creates a new [UnsafeTicketLock]
    pub const fn new() -> Self {
        Self {
            current_ticket: AtomicU64::new(0),
            next_ticket: AtomicU64::new(0),
            shattered: AtomicBool::new(false),
            owner: AtomicU16::new(!0),
            preemtable: true,
            _interrupt_state: PhantomData,
        }
    }

    /// creates a new non preemtable [UnsafeTicketLock]
    ///
    /// This assumes that it is save to disable interrupts
    /// while the lock is held.
    pub const fn new_non_preemtable() -> Self {
        Self {
            current_ticket: AtomicU64::new(0),
            next_ticket: AtomicU64::new(0),
            shattered: AtomicBool::new(false),
            owner: AtomicU16::new(!0),
            preemtable: false,
            _interrupt_state: PhantomData,
        }
    }

    /// write the "current" state of the ticket lock (not including the guarded data)
    /// to the `writer`.
    ///
    /// All internals are accessed with relaxed loads
    pub fn spew_state<W: core::fmt::Write>(&self, writer: &mut W) {
        let current = self.current_ticket.load(Ordering::Acquire);
        let next = self.next_ticket.load(Ordering::Acquire);
        let owner = self.owner.load(Ordering::Acquire);
        let shattered = self.shattered.load(Ordering::Acquire);
        let _ = writeln!(
            writer,
            "[UnsafeTicketLock(c: {current}, n: {next}, o: {owner}, s: {shattered})]"
        );
    }
}

impl<I: InterruptState> UnsafeTicketLock<I> {
    /// Blocks until lock is granted
    #[track_caller]
    pub fn lock(&self) -> LockReason {
        assert!(
            !self.preemtable || !I::s_in_interrupt(),
            "use of non-preemtable lock in interrupt"
        );
        if self.shattered.load(Ordering::Acquire) {
            return LockReason::Shattered;
        }

        unsafe {
            // Safety: disabling interrupts is ok, for preemtable locks
            I::s_enter_lock(!self.preemtable);
        }

        let ticket = self.next_ticket.fetch_add(1, Ordering::SeqCst);

        while self.current_ticket.load(Ordering::SeqCst) != ticket {
            let owner = self.owner.load(Ordering::Acquire);
            if owner != !0 && owner == I::s_core_id().0 as u16 {
                panic!("Deadlock detected. Lock already taken by this core");
            }

            while self.current_ticket.load(Ordering::Relaxed) != ticket {
                spin_loop();
            }
        }

        self.owner.store(I::s_core_id().0 as u16, Ordering::Release);

        LockReason::Acquired
    }

    /// Unlocks the lock
    ///
    /// # Safety
    ///
    /// must only be called if the current execution context holds the lock.
    #[track_caller]
    pub unsafe fn unlock(&self) {
        if cfg!(debug_assertions) {
            if self.is_unlocked() && !self.shattered.load(Ordering::Acquire) {
                panic!("Lock is already unlocked");
            }
        }

        self.owner.store(!0, Ordering::Release);
        self.current_ticket.fetch_add(1, Ordering::SeqCst);

        unsafe {
            I::s_exit_lock(!self.preemtable);
        }
    }

    /// "Permanently" shatters the lock. Following calls to [LockCell::lock] will
    /// succeed instantly.
    ///
    /// Some implementations might provide a way to reset the lock, or even reset themself
    /// automatically.
    ///
    /// # Safety
    ///
    /// the caller ensures that there is no "active" code that relies on this lock
    pub unsafe fn shatter_permanent(&self) {
        self.shattered.store(true, Ordering::SeqCst)
    }

    /// returns `true` if the LockCell is currently unlocked.
    ///
    /// However the caller can't relly on this fact, since some other
    /// core/interrupt etc could take the lock during or right after this call
    /// finishes.
    pub fn is_unlocked(&self) -> bool {
        self.owner.load(Ordering::Acquire) == !0
    }

    /// returns `true` if the lock is preemtable.
    ///
    /// In that case the lock is useable within interrupts, but must disable
    /// additional interrupts while being held.
    pub fn is_preemtable(&self) -> bool {
        self.preemtable
    }
}

/// A ticket lock implementation for [LockCell]
///
/// [TicketLock]s are preemtable and can be used within interrupts.
#[derive(Debug)]
pub struct TicketLock<T, I> {
    /// the data within the lock
    data: UnsafeCell<T>,
    /// the underlying usafe lock
    lock: UnsafeTicketLock<I>,
}

unsafe impl<T: Send, I: InterruptState> Send for TicketLock<T, I> {}
unsafe impl<T: Send, I: InterruptState> Sync for TicketLock<T, I> {}

impl<T, I> TicketLock<T, I> {
    /// creates a new [TicketLock]
    pub const fn new(data: T) -> Self {
        Self {
            data: UnsafeCell::new(data),
            lock: UnsafeTicketLock::new(),
        }
    }

    /// creates a new non preemtable [TicketLock]
    ///
    /// This assumes that it is save to disable interrupts
    /// while the lock is held.
    pub const fn new_non_preemtable(data: T) -> Self {
        Self {
            data: UnsafeCell::new(data),
            lock: UnsafeTicketLock::new_non_preemtable(),
        }
    }

    /// write the "current" state of the ticket lock (not including the guarded data)
    /// to the `writer`.
    ///
    /// All internals are accessed with relaxed loads
    pub fn spew_state<W: core::fmt::Write>(&self, writer: &mut W) {
        self.lock.spew_state(writer)
    }
}

impl<T: Send, I: InterruptState> LockCell<T> for TicketLock<T, I> {
    #[track_caller]
    fn lock(&self) -> LockCellGuard<'_, T, Self> {
        let lock_reason = self.lock.lock();

        // Safety: only this guard exists
        unsafe { LockCellGuard::new(self, lock_reason) }
    }
}

impl<T, I: InterruptState> LockCellInternal<T> for TicketLock<T, I> {
    unsafe fn get(&self) -> &T {
        unsafe { &*self.data.get() }
    }

    unsafe fn get_mut(&self) -> &mut T {
        unsafe { &mut *self.data.get() }
    }

    #[track_caller]
    unsafe fn unlock<'s, 'l: 's>(&'s self, guard: &mut LockCellGuard<'l, T, Self>) {
        assert!(core::ptr::eq(self, guard.lockcell));

        unsafe { self.unlock_no_guard() }
    }

    #[track_caller]
    unsafe fn unlock_no_guard(&self) {
        unsafe {
            self.lock.unlock();
        }
    }

    unsafe fn shatter_permanent(&self) {
        unsafe {
            self.lock.shatter_permanent();
        }
    }

    fn is_unlocked(&self) -> bool {
        self.lock.is_unlocked()
    }

    fn is_preemtable(&self) -> bool {
        self.lock.is_preemtable()
    }
}

impl<T: Default, I: InterruptState> Default for TicketLock<T, I> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T: Default, I: InterruptState> TicketLock<T, I> {
    /// creates a new preemtable [TicketLock] with `data` initialized to it's default
    ///
    /// This assumes that it is save to disable interrupts
    /// while the lock is held.
    pub fn default_non_preemtable() -> Self {
        Self::new_non_preemtable(Default::default())
    }
}

/// a [RWLockCell] implementation using a ticketing system.
pub struct ReadWriteCell<T, I> {
    /// If positive, this is the number of readers that currently hold a guard
    /// If 0, no one holds a guard, neither read nor write
    /// If -1, there is a writer with a guard
    access_count: AtomicI64,
    /// the data guarded by this lock
    data: UnsafeCell<T>,
    /// set if the lock is usable in interrupts
    pub preemtable: bool,
    /// phantom access to the cores interrupt state
    _interrupt_state: PhantomData<I>,
}

unsafe impl<T: Send, I: InterruptState> Send for ReadWriteCell<T, I> {}
unsafe impl<T: Send, I: InterruptState> Sync for ReadWriteCell<T, I> {}

impl<T, I> ReadWriteCell<T, I> {
    /// creates a new [ReadWriteCell]
    pub const fn new(data: T) -> Self {
        Self {
            access_count: AtomicI64::new(0),
            data: UnsafeCell::new(data),
            preemtable: true,
            _interrupt_state: PhantomData,
        }
    }

    /// creates a new non-preemtable [ReadWriteCell]
    ///
    /// This assumes that it is save to disable interrupts
    /// while the lock is held.
    pub const fn new_non_preemtable(data: T) -> Self {
        Self {
            access_count: AtomicI64::new(0),
            data: UnsafeCell::new(data),
            preemtable: false,
            _interrupt_state: PhantomData,
        }
    }
}

impl<T: Default, I> Default for ReadWriteCell<T, I> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T: Default, I> ReadWriteCell<T, I> {
    /// creates a new preemtable [ReadWriteCell] with `data` initialized to it's default
    ///
    /// This assumes that it is save to disable interrupts
    /// while the lock is held.
    pub fn default_non_preemtable() -> Self {
        Self::new_non_preemtable(Default::default())
    }
}

impl<T: Send, I: InterruptState> RWLockCell<T> for ReadWriteCell<T, I> {
    fn read(&self) -> ReadCellGuard<'_, T, Self> {
        // NOTE: because there can be multiple readers, RWLock is allowed
        // in interrupts even if preemtable
        unsafe {
            // Safety: disabling interrupts is false
            I::s_enter_lock(false);
        }

        let mut cur_count = self.access_count.load(Ordering::Acquire);
        loop {
            while cur_count < 0 {
                spin_loop();
                cur_count = self.access_count.load(Ordering::Acquire);
            }
            match self.access_count.compare_exchange(
                cur_count,
                cur_count + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(previous) => {
                    assert!(
                        previous >= 0,
                        "we managed to take a read lock even though the count is less than 0"
                    );
                    break;
                }
                Err(new_current) => cur_count = new_current,
            }
        }

        ReadCellGuard {
            rw_cell: self,
            _t: PhantomData,
        }
    }
}

impl<T: Send, I: InterruptState> LockCell<T> for ReadWriteCell<T, I> {
    fn lock(&self) -> LockCellGuard<'_, T, Self> {
        assert!(
            !self.preemtable || !I::s_in_interrupt(),
            "use onf non-preemtable lock in interrupt"
        );
        unsafe {
            // Safety: disabling interrupts is ok, for preemtable locks
            I::s_enter_lock(!self.preemtable);
        }

        loop {
            match self
                .access_count
                .compare_exchange(0, -1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(prev) => {
                    assert_eq!(prev, 0);
                    break;
                }
                Err(_) => spin_loop(),
            }
        }

        LockCellGuard {
            lockcell: self,
            reason: LockReason::Acquired,
            _t: PhantomData,
        }
    }
}

impl<T, I: InterruptState> RWCellInternal<T> for ReadWriteCell<T, I> {
    unsafe fn release_read<'s, 'l: 's>(&'s self, guard: &mut ReadCellGuard<'l, T, Self>) {
        assert!(core::ptr::eq(self, guard.rw_cell));

        unsafe { self.force_release_read() }
    }

    unsafe fn force_release_read(&self) {
        let previous_count = self.access_count.fetch_sub(1, Ordering::SeqCst);
        assert!(previous_count >= 1);
        // NOTE: read does not disable interrupts
        unsafe {
            I::s_exit_lock(false);
        }
    }

    fn open_to_read(&self) -> bool {
        self.access_count.load(Ordering::SeqCst) >= 0
    }
}

impl<T, I: InterruptState> LockCellInternal<T> for ReadWriteCell<T, I> {
    unsafe fn get(&self) -> &T {
        unsafe { &*self.data.get() }
    }

    unsafe fn get_mut(&self) -> &mut T {
        unsafe { &mut *self.data.get() }
    }

    unsafe fn unlock<'s, 'l: 's>(&'s self, guard: &mut LockCellGuard<'l, T, Self>) {
        assert!(core::ptr::eq(self, guard.lockcell));

        unsafe { self.unlock_no_guard() }
    }

    unsafe fn unlock_no_guard(&self) {
        let old_lock_value = self.access_count.swap(0, Ordering::SeqCst);
        assert_eq!(old_lock_value, -1);
        unsafe {
            // Safety: this will restore the interrupt state from when we called
            // enter_lock, so this is safe
            I::s_exit_lock(!self.preemtable);
        }
    }

    unsafe fn shatter_permanent(&self) {
        self.access_count.store(0, Ordering::SeqCst);
    }

    fn is_unlocked(&self) -> bool {
        self.access_count.load(Ordering::SeqCst) == 0
    }

    fn is_preemtable(&self) -> bool {
        self.preemtable
    }
}

/// A wrapper for a [`LockCell`] of an `MaybeUninit<T>`.
///
/// Just like a normal [LockCell], [`UnwrapLock::lock`] will return `T`.
/// This is unsafe and the user must ensure to initialize the value before
/// using it.
pub struct UnwrapLock<T: Send, L: LockCell<MaybeUninit<T>>> {
    /// inner lockcell that holds the `MaybeUninit<T>`
    pub lockcell: L,
    _t: PhantomData<T>,
}

/// creates a [UnwrapLock] wrapping type for the given [LockCell]
macro_rules! unwrapLockWrapper {
    (
        $(#[$outer:meta])*
        $lock_type:ident
    ) => {

        paste! {
            $(#[$outer])*
            pub type [<Unwrap $lock_type>]<T, I> = UnwrapLock<T, $lock_type<MaybeUninit<T>, I>>;

            impl<T: Send, I: InterruptState> [<Unwrap $lock_type>]<T, I> {
                /// creates a new [Self] that is uninitialized
                ///
                /// # Safety
                ///
                /// caller ensures that the [UnwrapLock] is initialized before it is accessed
                pub const unsafe fn new_uninit() -> Self {
                    unsafe {
                        UnwrapLock::new($lock_type::new(MaybeUninit::uninit()))
                    }
                }

                /// creates a new non preemtable [Self] that is uninitialized
                ///
                /// # Safety
                ///
                /// caller ensures that the [UnwrapLock] is initialized before it is accessed
                pub const unsafe fn new_non_preemtable_uninit() -> Self {
                    unsafe {
                        UnwrapLock::new($lock_type::new_non_preemtable(MaybeUninit::uninit()))
                    }
                }
            }
        }
    };
}

unwrapLockWrapper! {
    /// A [UnwrapLock] wrapper for [TicketLock]
    TicketLock
}

impl<T: Send, L: LockCell<MaybeUninit<T>>> Drop for UnwrapLock<T, L> {
    fn drop(&mut self) {
        unsafe {
            self.lock_uninit().assume_init_drop();
        }
    }
}

impl<T: Send, L: LockCell<MaybeUninit<T>> + Default> Default for UnwrapLock<T, L> {
    fn default() -> Self {
        Self {
            lockcell: Default::default(),
            _t: Default::default(),
        }
    }
}

unsafe impl<T: Send, L: LockCell<MaybeUninit<T>>> Send for UnwrapLock<T, L> {}
unsafe impl<T: Send, L: LockCell<MaybeUninit<T>>> Sync for UnwrapLock<T, L> {}

impl<T: Send, L: LockCell<MaybeUninit<T>>> UnwrapLock<T, L> {
    /// creates a new [UnwrapLock] from the given `inner` [LockCell]
    ///
    /// # Safety
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
        let reason = inner_guard.reason;
        core::mem::forget(inner_guard);
        unsafe { LockCellGuard::new(self, reason) }
    }
}

impl<T: Send, L: LockCell<MaybeUninit<T>>> LockCell<T> for UnwrapLock<T, L> {
    #[track_caller]
    fn lock(&self) -> LockCellGuard<'_, T, Self> {
        let inner_guard = self.lockcell.lock();
        let reason = inner_guard.reason;
        core::mem::forget(inner_guard);
        unsafe { LockCellGuard::new(self, reason) }
    }
}

impl<T: Send, L: LockCell<MaybeUninit<T>>> LockCellInternal<T> for UnwrapLock<T, L> {
    unsafe fn get(&self) -> &T {
        unsafe { self.lockcell.get().assume_init_ref() }
    }

    unsafe fn get_mut(&self) -> &mut T {
        unsafe { self.lockcell.get_mut().assume_init_mut() }
    }

    #[track_caller]
    unsafe fn unlock<'s, 'l: 's>(&'s self, guard: &mut LockCellGuard<'l, T, Self>) {
        assert!(core::ptr::eq(self, guard.lockcell));
        unsafe {
            self.lockcell.unlock_no_guard();
        }
    }

    #[track_caller]
    unsafe fn unlock_no_guard(&self) {
        unsafe {
            self.lockcell.unlock_no_guard();
        }
    }

    unsafe fn shatter_permanent(&self) {
        unsafe { self.lockcell.shatter_permanent() }
    }

    fn is_unlocked(&self) -> bool {
        self.lockcell.is_unlocked()
    }

    fn is_preemtable(&self) -> bool {
        self.lockcell.is_preemtable()
    }
}

impl<T: Send, L: LockCell<MaybeUninit<T>>> LockCellInternal<MaybeUninit<T>> for UnwrapLock<T, L> {
    unsafe fn get(&self) -> &MaybeUninit<T> {
        unsafe { self.lockcell.get() }
    }

    unsafe fn get_mut(&self) -> &mut MaybeUninit<T> {
        unsafe { self.lockcell.get_mut() }
    }

    unsafe fn unlock<'s, 'l: 's>(&'s self, _guard: &mut LockCellGuard<'l, MaybeUninit<T>, Self>) {
        unsafe {
            self.lockcell.unlock_no_guard();
        }
    }

    unsafe fn unlock_no_guard(&self) {
        unsafe {
            self.lockcell.unlock_no_guard();
        }
    }

    unsafe fn shatter_permanent(&self) {
        unsafe { self.lockcell.shatter_permanent() }
    }

    fn is_unlocked(&self) -> bool {
        self.lockcell.is_unlocked()
    }

    fn is_preemtable(&self) -> bool {
        self.lockcell.is_preemtable()
    }
}

impl<T: Send, L: RWLockCell<MaybeUninit<T>>> RWLockCell<T> for UnwrapLock<T, L> {
    fn read(&self) -> ReadCellGuard<'_, T, Self> {
        let inner_guard = self.lockcell.read();
        core::mem::forget(inner_guard);
        unsafe { ReadCellGuard::new(self) }
    }
}

impl<T: Send, L: RWLockCell<MaybeUninit<T>>> RWCellInternal<T> for UnwrapLock<T, L> {
    unsafe fn release_read<'s, 'l: 's>(&'s self, guard: &mut ReadCellGuard<'l, T, Self>) {
        assert!(core::ptr::eq(self, guard.rw_cell));
        unsafe {
            self.force_release_read();
        }
    }

    fn open_to_read(&self) -> bool {
        self.lockcell.open_to_read()
    }
}
