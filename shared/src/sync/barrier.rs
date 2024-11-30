//! Module containing barriers.
//!
//! Barriers allow multiple processors to syncronize by waiting for all processors
//! to enter pass the barrier at the same time
use core::{
    fmt::{self, Debug},
    hint::spin_loop,
    ops::Deref,
    sync::atomic::{AtomicU32, Ordering},
};

use thiserror::Error;

use crate::{
    cpu::time::timestamp_now_tsc,
    types::{TscDuration, TscTimestamp},
};

use super::{
    lockcell::{RWLockCell, ReadCellGuard, ReadWriteCell},
    InterruptState,
};

/// A barrier that only allows entry when [Self::target]
/// processors are waiting.
/// This will accept exactly [Self::target] processors and autmatically reset.
pub struct Barrier {
    enter_count: AtomicU32,
    passing_count: AtomicU32,
    /// The number of processors that need to wait for all to enter
    target: AtomicU32,
    /// used to access [Self::target]
    ordering: Ordering,
}

impl Barrier {
    /// Create a new barrier which waits for `target` processors
    pub const fn new(target: u32) -> Self {
        Self {
            enter_count: AtomicU32::new(0),
            passing_count: AtomicU32::new(0),
            target: AtomicU32::new(target),
            ordering: Ordering::Relaxed,
        }
    }

    /// Change the ordering used to read [Self::target]
    pub const fn with_target_ordering(mut self, ordering: Ordering) -> Self {
        self.ordering = ordering;
        self
    }

    /// the number of processors that are currently waiting at the barrier
    pub fn target(&self) -> u32 {
        self.target.load(self.ordering)
    }

    /// set the number of processors that are currently waiting at the barrier
    ///
    /// # Safety
    ///
    /// Caller must guarantee that changing the target won't invalidate the barrier for
    /// waiting processors. This generally means that this has to be called when only 1
    /// processor has access to the barrier and the barrier is not yet in use.
    pub unsafe fn set_target(&self, target: u32, ordering: Ordering) {
        self.target.store(target, ordering)
    }

    /// Wait until [Self::target] processors are waiting.
    ///
    /// Returns `true` for the leader, aka the first processor to reach
    /// the barrier.
    pub fn enter(&self) -> bool {
        let target = self.target.load(self.ordering);

        loop {
            // the barrier is currently open for the last set of processors
            let pass_count = self.passing_count.load(Ordering::SeqCst);
            if pass_count == target || pass_count == 0 {
                break;
            }
            spin_loop();
        }

        // try to restet passing_count to target.
        // The processor that succeeds is the leader that is responsible to
        // wait for all other processors to pass and reset the barrier
        let leader = self
            .passing_count
            .compare_exchange(0, target, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();

        // wait for all processors to reach the barrier
        let mut current = self.enter_count.fetch_add(1, Ordering::SeqCst);
        while current < target {
            spin_loop();
            current = self.enter_count.load(Ordering::SeqCst);
        }

        if leader {
            // leader waits for all processors to pass the barrier before resetting it by storing 0
            // in enter_count and pass_count
            while self.passing_count.load(Ordering::SeqCst) != 1 {
                spin_loop();
            }
            self.enter_count.store(0, Ordering::SeqCst);
            self.passing_count.store(0, Ordering::SeqCst);
        } else {
            self.passing_count.fetch_sub(1, Ordering::SeqCst);
        }
        leader
    }

    /// Wait until [Self::target] processors are waiting or timeout is reached.
    ///
    /// calling enter after a timeout might cause a deadlock on [Self::enter] and similar
    /// unless all other currently waiting processors also time out.
    pub fn enter_with_timeout(&self, timeout: TscDuration) -> Result<bool, TscDuration> {
        let mut start = TscTimestamp::new(0);
        let mut end = start;
        // tsc time is slow, so we can skip this if we take only a few iterations
        let mut timeout_iters = 10_000;
        let mut check_timeout = || {
            if timeout_iters == 0 {
                let now = timestamp_now_tsc();
                if now > end {
                    return Err(now - start);
                }
            } else {
                timeout_iters -= 1;
                if timeout_iters == 0 {
                    start = timestamp_now_tsc();
                    end = start + timeout;
                }
            }
            Ok(())
        };

        let target = self.target.load(self.ordering);

        loop {
            // the barrier is currently open for the last set of processors
            let pass_count = self.passing_count.load(Ordering::SeqCst);
            if pass_count == target || pass_count == 0 {
                break;
            }

            check_timeout()?;

            spin_loop();
        }

        // try to restet passing_count to target.
        // The processor that succeeds is the leader that is responsible to
        // wait for all other processors to pass and reset the barrier
        let leader = self
            .passing_count
            .compare_exchange(0, target, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();

        // wait for all processors to reach the barrier
        let mut current = self.enter_count.fetch_add(1, Ordering::SeqCst);
        while current < target {
            if let Err(timeout) = check_timeout() {
                if self
                    .enter_count
                    .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    return Err(timeout);
                }
            }

            spin_loop();
            current = self.enter_count.load(Ordering::SeqCst);
        }

        if leader {
            // in enter_count and pass_count
            while self.passing_count.load(Ordering::SeqCst) != 1 {
                check_timeout()?;
                spin_loop();
            }
            self.enter_count.store(0, Ordering::SeqCst);
            self.passing_count.store(0, Ordering::SeqCst);
        } else {
            self.passing_count.fetch_sub(1, Ordering::SeqCst);
        }

        Ok(leader)
    }

    /// Shatter the barrier and let all waiting processors procede
    ///
    /// Can be undone using [Self::reset]
    ///
    /// # Safety
    ///
    /// the caller must guarantee that shattering does not create UB
    pub unsafe fn shatter(&self) {
        let target = self.target.load(self.ordering);
        self.enter_count.store(target, Ordering::Release);
    }
}

/// A [Barrier] that allows 1 processor to pass data along to all other processors.
pub struct DataBarrier<T, I> {
    data: ReadWriteCell<Option<T>, I>,
    entry: Barrier,
}

/// A guard that allows access to the value passed into a [DataBarrier]
pub struct DataBarrierGuard<'l, T, I: InterruptState> {
    data: ReadCellGuard<'l, Option<T>, ReadWriteCell<Option<T>, I>>,
    leader: bool,
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum DataBarrierError {
    #[error("No data provided")]
    NoData,
    #[error("barrier timed out after {} ticks.", duration.as_i64())]
    Timeout { duration: TscDuration },
}

impl<T, I> DataBarrier<T, I> {
    /// Create a new barrier that waits for `target` processors
    pub const fn new(target: u32) -> Self {
        Self {
            data: ReadWriteCell::new(None),
            entry: Barrier::new(target),
        }
    }

    /// Create a new barrier that waits for `target` processors
    ///
    /// This assumes that it is save to disable interrupts while the data
    /// within this barrier is acessed.
    pub const fn new_non_preemtable(target: u32) -> Self {
        Self {
            data: ReadWriteCell::new_non_preemtable(None),
            entry: Barrier::new(target),
        }
    }

    /// Change the ordering that the [target] is read with
    pub const fn with_target_ordering(mut self, ordering: Ordering) -> Self {
        self.entry = self.entry.with_target_ordering(ordering);
        self
    }

    /// the number of processors that are currently waiting at the barrier
    pub fn target(&self) -> u32 {
        self.entry.target()
    }

    /// set the number of processors that are currently waiting at the barrier
    ///
    /// # Safety
    ///
    /// Caller must guarantee that changing the target won't invalidate the barrier for
    /// waiting processors
    pub unsafe fn set_target(&self, target: u32, ordering: Ordering) {
        self.entry.set_target(target, ordering)
    }
}

impl<T: Send, I: InterruptState> DataBarrier<T, I> {
    /// Calls [Barrier::enter]
    pub fn enter(&self) -> Result<DataBarrierGuard<'_, T, I>, DataBarrierError> {
        let leader = self.entry.enter();

        let data_guard = self.data.read();
        if data_guard.is_none() {
            return Err(DataBarrierError::NoData);
        }
        Ok(DataBarrierGuard {
            data: data_guard,
            leader,
        })
    }

    /// Calls [Barrier::enter_with_timeout]
    pub fn enter_with_timeout(
        &self,
        timeout: TscDuration,
    ) -> Result<DataBarrierGuard<'_, T, I>, DataBarrierError> {
        let leader = match self.entry.enter_with_timeout(timeout) {
            Err(duration) => return Err(DataBarrierError::Timeout { duration }),
            Ok(leader) => leader,
        };

        let data_guard = self.data.read();
        if data_guard.is_none() {
            return Err(DataBarrierError::NoData);
        }
        Ok(DataBarrierGuard {
            data: data_guard,
            leader,
        })
    }

    /// Same as [DataBarrier::enter] but also sets the data returned by the [DataBarrierGuard].
    pub fn enter_with_data(&self, data: T) -> Result<DataBarrierGuard<'_, T, I>, DataBarrierError> {
        {
            // block to ensure that we drop the write lock before entering
            *self.data.write() = Some(data);
        }

        self.enter()
    }

    /// Same as [DataBarrier::enter_with_timeout] but also sets the data returned by the [DataBarrierGuard].
    pub fn enter_with_data_timeout(
        &self,
        data: T,
        timeout: TscDuration,
    ) -> Result<DataBarrierGuard<'_, T, I>, DataBarrierError> {
        {
            // block to ensure that we drop the write lock before entering
            *self.data.write() = Some(data);
        }

        self.enter_with_timeout(timeout)
    }

    /// Takes the data out of the barrier if any exists.
    ///
    /// This will block until all [DataBarrierGuard] associated with this
    /// barrier are dropped.
    pub fn take_data(&self) -> Option<T> {
        self.data.write().take()
    }
}

impl<T, I: InterruptState> DataBarrierGuard<'_, T, I> {
    /// returns `true` if this was created on the processor that first
    /// reached the barrier
    pub fn is_leader(&self) -> bool {
        self.leader
    }
}

impl<T, I: InterruptState> AsRef<T> for DataBarrierGuard<'_, T, I> {
    fn as_ref(&self) -> &T {
        self.data.as_ref().unwrap()
    }
}

impl<T, I: InterruptState> Deref for DataBarrierGuard<'_, T, I> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T: Debug, I: InterruptState> Debug for DataBarrierGuard<'_, T, I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl<T: fmt::Display, I: InterruptState> fmt::Display for DataBarrierGuard<'_, T, I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}
