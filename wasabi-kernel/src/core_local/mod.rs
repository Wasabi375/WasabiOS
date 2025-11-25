//! Module containing data/access to core local structures
//!
//! This provides the [locals!] macro as well as [CoreStatics] struct
//! which can be used to access per core static kernel data.

mod statics;
pub use statics::{CoreStatics, InterruptDisableGuard, core_boot, get_core_statics, init};

/// A [shared::task_local::TaskLocal] based on [CoreId]
pub type CoreLocal<T> = shared::task_local::TaskLocal<T, CoreInterruptState>;

/// A [shared::task_local::TaskLocalRef] ref based on [CoreId]
pub type CoreLocalRef<'l, T> = shared::task_local::TaskLocalRef<'l, T, CoreInterruptState>;

#[allow(unused_imports)]
use log::{debug, info, trace, warn};

#[cfg(feature = "test")]
use crate::test_locals;

use crate::locals;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use shared::{
    sync::{CoreInfo, InterruptState},
    types::CoreId,
};

/// A counter used to assign the core id for each core.
/// Each core calls [AtomicU8::fetch_add] to get it's id and automatically increment
/// it for the next core, ensuring ids are unique.
///
/// As a side-effect, this is also the number of cores that have been started
static CORE_ID_COUNTER: AtomicU8 = AtomicU8::new(0);

/// The number of cores that have finished booting
static CORE_READY_COUNT: AtomicU8 = AtomicU8::new(0);

/// An reference counter that automatically decrements using a [AutoRefCounterGuard].
#[derive(Debug)]
pub struct AutoRefCounter(AtomicU64);

// TODO check Ordering. right now AutoRefCounter uses SeqCst which is fine, but I can prob relax
// this
impl AutoRefCounter {
    /// creates a new [AutoRefCounter]
    pub const fn new(init: u64) -> Self {
        Self(AtomicU64::new(init))
    }

    /// returns the count
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    /// increments the count and returns a [AutoRefCounterGuard], which will
    /// decrement the count on [Drop].
    pub fn increment(&self) -> AutoRefCounterGuard<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        AutoRefCounterGuard(self)
    }

    /// decrements the count
    ///
    /// # Safety
    ///
    /// This is an escape hatch to decrement when a [AutoRefCounterGuard] can not be dropped.
    /// The caller must ensure that decrementing the counter does not break any other external
    /// safety guarantees
    pub unsafe fn decrement(&self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Guard struct which will decrement the count of the associated [AutoRefCounter].
pub struct AutoRefCounterGuard<'a>(&'a AutoRefCounter);

impl<'a> Drop for AutoRefCounterGuard<'a> {
    fn drop(&mut self) {
        (self.0).0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A ZST used to access the interrupt state of this core.
///
/// See [InterruptState]
pub struct CoreInterruptState;

impl CoreInfo for CoreInterruptState {
    fn core_id(&self) -> CoreId {
        locals!().core_id
    }

    fn is_bsp(&self) -> bool {
        locals!().is_bsp()
    }

    fn is_initialized(&self) -> bool {
        locals!().initialized
    }

    fn instance() -> Self
    where
        Self: Sized,
    {
        CoreInterruptState
    }

    fn max_core_count() -> u8
    where
        Self: Sized,
    {
        get_ready_core_count(Ordering::SeqCst)
    }

    fn task_system_is_init(&self) -> bool {
        locals!().task_system.is_init.load(Ordering::Acquire)
    }

    unsafe fn write_current_task_name(
        &self,
        writer: &mut dyn core::fmt::Write,
    ) -> Result<(), core::fmt::Error> {
        let handle = locals!().task_system.current_task(Ordering::Relaxed);

        writer.write_fmt(format_args!("({}", handle.to_u64()))?;

        #[cfg(feature = "log-task-name")]
        if locals!()
            .task_system
            .log_task_locked_data
            .load(Ordering::Relaxed)
        {
            let Some(info) = locals!().task_system.get_task_info(handle) else {
                return Ok(());
            };

            if let Some(name) = info.name {
                writer.write_str(": ")?;
                writer.write_str(name)?;
            } else {
                writer.write_str(": unknown")?;
            }
        }

        writer.write_char(')')?;

        Ok(())
    }
}

impl InterruptState for CoreInterruptState {
    #[track_caller]
    #[inline(always)]
    fn in_interrupt(&self) -> bool {
        locals!().in_interrupt()
    }

    #[track_caller]
    #[inline(always)]
    fn in_exception(&self) -> bool {
        locals!().in_exception()
    }

    unsafe fn enter_lock(&self, disable_interrupts: bool) {
        #[cfg(feature = "test")]
        test_locals!().lock_count.fetch_add(1, Ordering::AcqRel);

        if disable_interrupts {
            unsafe {
                // safety: disbaling interrupts is ok for locked critical sections
                locals!().disable_interrupts_guardless();
            }
        }
    }

    unsafe fn exit_lock(&self, enable_interrupts: bool) {
        #[cfg(feature = "test")]
        test_locals!().lock_count.fetch_sub(1, Ordering::AcqRel);

        if enable_interrupts {
            unsafe {
                // safety: only called once, when a lock-guard is dropped
                locals!().enable_interrupts();
            }
        }
    }
}

/// The number of cores that have started
///  
/// This is the number of cores that started their boot sequence.
/// For most situation [get_ready_core_count] is the more accurate function.
pub fn get_started_core_count(ordering: Ordering) -> u8 {
    CORE_ID_COUNTER.load(ordering)
}

/// The number of cores that finished booting
pub fn get_ready_core_count(ordering: Ordering) -> u8 {
    CORE_READY_COUNT.load(ordering)
}

/// A macro wrapper around [get_core_locals] returning this' core [CoreStatics] struct.
///
/// # Safety
///
/// This assumes that `GS` segement was initialized by [init] to point to
/// the [CoreStatics] struct for this core.
///
/// This macro includes the necessary unsafe block to allow calling this from safe
/// rust, but it's still unsafe before [core_boot] and/or [init] have been called.
#[macro_export]
macro_rules! locals {
    () => {{
        #[allow(unused_unsafe)]
        let locals = unsafe { $crate::core_local::get_core_statics() };

        locals
    }};
}
