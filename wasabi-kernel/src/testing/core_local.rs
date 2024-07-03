//! Core locals for kernel tests

use core::sync::atomic::{AtomicU32, Ordering};

use alloc::boxed::Box;
use shared::sync::lockcell::TicketLock;
use testing::multiprocessor::TestInterruptState;

use super::panic::CustomPanicHandler;

/// struct containing core local data for tests
#[repr(C)]
pub struct TestCoreLocals {
    /// The number of locks, currently held
    pub lock_count: AtomicU32,

    /// holds the custom panic handler if it exists
    pub custom_panic: TicketLock<Option<Box<CustomPanicHandler>>, TestInterruptState>,
}

impl TestCoreLocals {
    /// creates a new [TestCoreLocals] struct
    pub const fn new() -> Self {
        Self {
            lock_count: AtomicU32::new(0),
            custom_panic: TicketLock::new(None),
        }
    }

    /// Returns `true` if any lock is held
    pub fn in_lock(&self) -> bool {
        self.lock_count.load(Ordering::Acquire) != 0
    }
}

/// Returns this' core [TestCoreLocals] struct.
///
/// # Safety
///
/// This assumes that `GS` segement was initialized by
/// [init](crate::core_local::init) to point to the
/// [CoreLocals](crate::core_local::CoreLocals) struct for this core.
///
/// see [get_core_locals](crate::core_local::get_core_locals)
pub unsafe fn get_test_core_locals() -> &'static TestCoreLocals {
    unsafe { &crate::core_local::get_core_locals().test_local }
}

/// A macro wrapper around [get_test_core_locals] returning this' core [TestCoreLocals] struct.
///
/// # Safety
///
/// This assumes that `FS` segement was initialized to point to
/// the [TestCoreLocals] struct for this core.
///
/// This macro includes the necessary unsafe block to allow calling this from
/// safe rust, but it's still unsafe before
/// [core_boot](crate::core_local::core_boot) and/or
/// [init](crate::core_local::init) have been called.
#[macro_export]
macro_rules! test_locals {
    ($i:ty) => {{
        #[allow(unused_unsafe)]
    }};
    () => {{
        let locals: &$crate::testing::core_local::TestCoreLocals =
            unsafe { $crate::testing::core_local::get_test_core_locals() };
        locals
    }};
}
