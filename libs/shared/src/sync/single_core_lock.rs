//! Provides a single core lock, that ensures that a task is completed
//! on the current core, before any context switching can occure

use core::marker::PhantomData;

use crate::types::{NotSend, NotSync};

use super::InterruptState;

/// A single core lock
///
/// This can be used to lock the current Core to this "task" ensuring
/// no context switching can be done.
#[derive(Debug, Default)]
pub struct SingleCoreLock<I: InterruptState> {
    _not_send: NotSend,
    _not_sync: NotSync,
    _interrupt_state: PhantomData<I>,
}

impl<I: InterruptState> SingleCoreLock<I> {
    /// Creates a new [SingleCoreLock]
    pub const fn new() -> Self {
        SingleCoreLock {
            _not_send: NotSend,
            _not_sync: NotSync,
            _interrupt_state: PhantomData,
        }
    }

    /// Locks the current core to this "task"
    ///
    /// No context switching can occure until the returned [SingleCoreLockGuard]
    /// is dropped.
    pub fn lock(&mut self) -> SingleCoreLockGuard<'_, I> {
        unsafe {
            // This should act as a barrier, because it disables interrupts
            I::s_enter_lock(true);
        }
        SingleCoreLockGuard { lock: self }
    }

    fn unlock(&mut self) {
        unsafe {
            I::s_exit_lock(true);
        }
    }
}

/// A guard that ensures no context switching occures while held
///
/// See [SingleCoreLock]
pub struct SingleCoreLockGuard<'l, I: InterruptState> {
    lock: &'l mut SingleCoreLock<I>,
}

impl<I: InterruptState> Drop for SingleCoreLockGuard<'_, I> {
    fn drop(&mut self) {
        self.lock.unlock()
    }
}
