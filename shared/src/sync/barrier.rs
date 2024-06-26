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

use crate::{cpu::time::timestamp_now_tsc, types::TscDuration};

use super::{
    lockcell::{RWLockCell, ReadCellGuard, ReadWriteCell},
    InterruptState,
};

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
pub struct BarrierTryError {
    /// the number of processors that have yet to enter the barrier
    pub waiting_on: u32,
    /// the number of processors that are currently waiting at the barrier
    pub waiting: u32,
}

impl Barrier {
    /// Create a new barrier that waits for `target` processors
    pub const fn new(target: u32) -> Self {
        Self {
            count: AtomicU32::new(0),
            target,
        }
    }

    /// resets the barrier.
    ///
    /// This fails if there are currently any waiting processors.
    pub fn reset(&self) -> Result<(), BarrierTryError> {
        if self.count.load(Ordering::Acquire) == 0 {
            return Ok(());
        }
        match self
            .count
            .compare_exchange(self.target, 0, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(()),
            Err(waiting) => Err(BarrierTryError {
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

    /// Wait until [Self::target] processors are waiting or timeout is reached.
    pub fn enter_with_timeout(
        &self,
        timeout: TscDuration,
    ) -> Result<(), (TscDuration, BarrierTryError)> {
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
                        BarrierTryError {
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

/// A [Barrier] that allows 1 processor to pass data along to all other processors.
pub struct DataBarrier<T, I> {
    data: ReadWriteCell<Option<T>, I>,
    entry: Barrier,
    // the number of processors that could potentially read data
    critical: AtomicU32,
}

/// A guard that allows access to the value passed into a [DataBarrier]
pub struct DataBarrierGuard<'l, T, I: InterruptState> {
    data: ReadCellGuard<'l, Option<T>, ReadWriteCell<Option<T>, I>>,
    barrier: &'l DataBarrier<T, I>,
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum DataBarrierError {
    #[error("No data provided")]
    NoData,
    #[error("barrier timed out after {} ticks. {:?}", duration.as_i64(), failure)]
    Timeout {
        duration: TscDuration,
        failure: BarrierTryError,
    },
    #[error("failed to reset barrier")]
    Reset(BarrierTryError),
    #[error("{0} processors are still accessing the old data")]
    DataStillAccessed(u32),
}

impl<T, I> DataBarrier<T, I> {
    /// Create a new barrier that waits for `target` processors
    pub fn new(target: u32) -> Self {
        Self {
            data: ReadWriteCell::new(None),
            entry: Barrier::new(target),
            critical: AtomicU32::new(0),
        }
    }

    /// Create a new barrier that waits for `target` processors
    ///
    /// This assumes that it is save to disable interrupts while the data
    /// within this barrier is acessed.
    pub fn new_non_preemtable(target: u32) -> Self {
        Self {
            data: ReadWriteCell::new_non_preemtable(None),
            entry: Barrier::new(target),
            critical: AtomicU32::new(0),
        }
    }
}

impl<T: Send, I: InterruptState> DataBarrier<T, I> {
    /// reset the data barrier
    pub fn reset(&self) -> Result<Option<T>, DataBarrierError> {
        let mut data = self.data.write();

        let access_count = self.critical.load(Ordering::Acquire);
        if access_count > 0 {
            return Err(DataBarrierError::DataStillAccessed(access_count));
        }

        self.entry.reset().map_err(|e| DataBarrierError::Reset(e))?;

        Ok(data.take())
    }

    /// Calls [Barrier::enter]
    pub fn enter(&self) -> Result<DataBarrierGuard<'_, T, I>, DataBarrierError> {
        self.entry.enter();

        self.critical.fetch_add(1, Ordering::SeqCst);

        let data_guard = self.data.read();
        if data_guard.is_none() {
            return Err(DataBarrierError::NoData);
        }
        Ok(DataBarrierGuard {
            data: data_guard,
            barrier: self,
        })
    }

    /// Calls [Barrier::enter_with_timeout]
    pub fn enter_with_timeout(
        &self,
        timeout: TscDuration,
    ) -> Result<DataBarrierGuard<'_, T, I>, DataBarrierError> {
        if let Err((duration, failure)) = self.entry.enter_with_timeout(timeout) {
            return Err(DataBarrierError::Timeout { duration, failure });
        }

        self.critical.fetch_add(1, Ordering::SeqCst);

        let data_guard = self.data.read();
        if data_guard.is_none() {
            return Err(DataBarrierError::NoData);
        }
        Ok(DataBarrierGuard {
            data: data_guard,
            barrier: self,
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
}

impl<'l, T, I: InterruptState> Drop for DataBarrierGuard<'l, T, I> {
    fn drop(&mut self) {
        self.barrier.critical.fetch_sub(1, Ordering::SeqCst);
    }
}

impl<'l, T, I: InterruptState> AsRef<T> for DataBarrierGuard<'l, T, I> {
    fn as_ref(&self) -> &T {
        self.data.as_ref().unwrap()
    }
}

impl<'l, T, I: InterruptState> Deref for DataBarrierGuard<'l, T, I> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<'l, T: Debug, I: InterruptState> Debug for DataBarrierGuard<'l, T, I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl<'l, T: fmt::Display, I: InterruptState> fmt::Display for DataBarrierGuard<'l, T, I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}
