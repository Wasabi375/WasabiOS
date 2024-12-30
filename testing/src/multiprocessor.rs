//! Multiprocessor functionality for testing

use shared::sync::{CoreInfo, InterruptState};

/// type alias for [DataBarrier](shared::sync::barrier::DataBarrier) using [TestInterruptState]
pub type DataBarrier<T> = shared::sync::barrier::DataBarrier<T, TestInterruptState>;

/// [InterruptState] used by testing.
///
/// This implementation will panic if used in most cases.
/// Only `enter_lock` and `exit_lock` are supported as long as the interrupt state
/// is not accessed.
pub struct TestInterruptState;

static mut INTERRUPT_STATE: Option<&'static dyn InterruptState> = None;

/// initializes interrupts and locks for testing
/// This must be called before test execution starts
pub fn init_interrupt_state(interrupt_state: &'static dyn InterruptState) {
    unsafe {
        INTERRUPT_STATE = Some(interrupt_state);
    }
}

#[track_caller]
#[inline(always)]
fn interrupt_state() -> &'static dyn InterruptState {
    unsafe { INTERRUPT_STATE.expect("Test interrupt state not initialized") }
}

impl CoreInfo for TestInterruptState {
    fn core_id(&self) -> shared::types::CoreId {
        interrupt_state().core_id()
    }

    fn is_bsp(&self) -> bool {
        interrupt_state().is_bsp()
    }

    fn instance() -> Self
    where
        Self: Sized,
    {
        TestInterruptState
    }

    fn is_initialized(&self) -> bool {
        interrupt_state().is_initialized()
    }
}

impl InterruptState for TestInterruptState {
    #[track_caller]
    #[inline(always)]
    fn in_interrupt(&self) -> bool {
        interrupt_state().in_interrupt()
    }

    #[track_caller]
    #[inline(always)]
    fn in_exception(&self) -> bool {
        interrupt_state().in_exception()
    }

    #[track_caller]
    #[inline(always)]
    unsafe fn enter_lock(&self, disable_interrupts: bool) {
        unsafe { interrupt_state().enter_lock(disable_interrupts) }
    }

    #[track_caller]
    #[inline(always)]
    unsafe fn exit_lock(&self, enable_interrupts: bool) {
        unsafe { interrupt_state().exit_lock(enable_interrupts) }
    }
}
