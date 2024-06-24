//! Module containing syncronization primitives
//!

use core::{
    hint::spin_loop,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::{
    cpu::time::timestamp_now_tsc,
    types::{CoreId, TscDuration},
};

pub mod lockcell;

/// Trait that allows access to OS-level constructs defining basic information
/// about the current processor.
pub trait CoreInfo: 'static {
    /// Gets the ID of the running core. It's required that this core ID is
    /// unique to the core.
    fn core_id() -> CoreId;

    /// Returns `true` if the current processor is the bootstrap processor.
    fn is_bsp() -> bool;

    /// Returns `true` if the current processor is an application processor.
    ///
    /// This is `true`  if [Self::is_bsp] is `false`
    fn is_ap() -> bool {
        !Self::is_bsp()
    }
}

/// Trait that allows access to OS-level constructs defining interrupt state,
/// exception state, unique core IDs, and enter/exit lock (for interrupt
/// disabling and enabling) primitives.
pub trait InterruptState: CoreInfo + 'static {
    /// Returns `true` if we're currently in an interrupt
    fn in_interrupt() -> bool;

    /// Returns `true` if we're currently in an exception. Which indicates that
    /// a lock cannot be held as we may have pre-empted a non-preemptable lock
    fn in_exception() -> bool;

    /// Signal the kernel that a lock was taken. If `disable_interrupts` the
    /// lock does not support being interrupted and therefor we must disable
    /// interrupts. This is also a prequisite for a lock to be taken within an
    /// interrupt.
    ///
    /// # Safety:
    ///
    /// * Caller must call [InterruptState::exit_lock] exactly once with the
    ///     same parameter for `enable_interrupts`
    /// * If `disable_interrupts` caller must ensure that interrupts can be
    ///     disabled safely
    unsafe fn enter_lock(disable_interrupts: bool);

    /// Signal the kernel that a lock was released. If `enable_interrupts` the
    /// kernel will reenable interrupts if possible.
    ///
    /// # Safety:
    ///
    /// * caller must ensure that this function is called exactly once per invocation
    ///     of [InterruptState::enter_lock] with the same parameter.
    unsafe fn exit_lock(enable_interrupts: bool);

    /// returns the instance of this interrupt state. This should always be a zst.
    fn instance() -> Self;
}

/// A barrier that only allows entry when [Self::target]
/// processors are waiting
pub struct Barrier {
    count: AtomicU32,
    /// The number of processors that need to wait for all to enter
    pub target: u32,
}

/// Failure of a barrier try operation
///
/// the contained values might not be correct, since they might have
/// changed since they were read
#[derive(Debug, Clone, Copy)]
pub struct BarrierTryFailure {
    /// the number of processors that have yet to enter the barrier
    pub waiting_on: u32,
    /// the number of processors that are currently waiting at the barrier
    pub waiting: u32,
}

impl Barrier {
    pub const fn new(target: u32) -> Self {
        Self {
            count: AtomicU32::new(0),
            target,
        }
    }

    /// resets the barrier.
    ///
    /// This fails if there are currently any waiting processors.
    pub fn reset(&self) -> Result<(), BarrierTryFailure> {
        if self.count.load(Ordering::Acquire) == 0 {
            return Ok(());
        }
        match self
            .count
            .compare_exchange(self.target, 0, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(()),
            Err(waiting) => Err(BarrierTryFailure {
                waiting,
                waiting_on: self.target - waiting,
            }),
        }
    }

    /// returns true if the barrier can instantly be passed.
    ///
    /// Not that between this call and [Self::enter] another processor might call [Self::reset]
    pub fn is_open(&self) -> bool {
        self.count.load(Ordering::Acquire) >= self.target
    }

    /// Wait until [Self::target] processors are waiting
    pub fn enter(&self) {
        let mut current = self.count.fetch_add(1, Ordering::SeqCst);
        while current < self.target {
            spin_loop();
            current = self.count.load(Ordering::SeqCst);
        }
    }

    pub fn enter_with_timeout(
        &self,
        timeout: TscDuration,
    ) -> Result<(), (TscDuration, BarrierTryFailure)> {
        let start = timestamp_now_tsc();
        let end = start + timeout;
        let mut current = self.count.fetch_add(1, Ordering::SeqCst);
        while current < self.target {
            spin_loop();

            let now = timestamp_now_tsc();
            if now >= end {
                // try to reduce count by 1. We cant use fetch_sub because
                // some other processor might have entered since we last read count
                // and therefor we might be done waiting.
                if self
                    .count
                    .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    // we successfuly marked us as no longer waiting, we can return
                    return Err((
                        now - start,
                        BarrierTryFailure {
                            waiting_on: self.target - (current - 1),
                            waiting: current - 1,
                        },
                    ));
                }
            }
            current = self.count.load(Ordering::SeqCst);
        }
        Ok(())
    }

    /// Shatter the barrier and let all waiting processors procede
    ///
    /// Can be undone using [Self::reset]
    ///
    /// # Safety
    ///
    /// the caller must guarantee that shattering does not create UB
    pub unsafe fn shatter(&self) {
        self.count.store(self.target, Ordering::Release);
    }
}
