//! Multiprocessor functionality for testing

use core::{
    hint::spin_loop,
    ptr::{addr_of, addr_of_mut, null, null_mut},
    sync::atomic::{AtomicPtr, AtomicU8, Ordering},
};

use shared::sync::{CoreInfo, InterruptState};

/// type alias for [DataBarrier](shared::sync::barrier::DataBarrier) using [TestInterruptState]
pub type DataBarrier<T> = shared::sync::barrier::DataBarrier<T, TestInterruptState>;

/// [InterruptState] used by testing.
///
/// This implementation is a wrapper around a static interrupt state provided by calling
/// [init_interrupt_state]
pub struct TestInterruptState;

static mut INTERRUPT_STATE: Option<&'static dyn InterruptState> = None;
static mut INTERRUPT_STATE_INITIALIZING_DUMMY: Option<&'static dyn InterruptState> = None;

static INTERRUPT_STATE_GUARD_PTR: AtomicPtr<Option<&'static dyn InterruptState>> =
    AtomicPtr::new(null_mut());

static MAX_CORE_COUNT: AtomicU8 = AtomicU8::new(0);

/// initializes interrupts and locks for testing
///
/// # Safety
///
/// This must be called before test execution starts on every core/thread
///
/// If called multiple times this function assumes that the arguments to any call
/// are the same as the first and will not do anything.
pub unsafe fn init_interrupt_state(
    interrupt_state: &'static dyn InterruptState,
    max_core_count: u8,
) {
    let interrupt_state_ptr = addr_of_mut!(INTERRUPT_STATE);
    let dummy_ptr = addr_of_mut!(INTERRUPT_STATE_INITIALIZING_DUMMY);

    let mut guard_ptr = match INTERRUPT_STATE_GUARD_PTR.compare_exchange(
        null_mut(),
        dummy_ptr,
        Ordering::Relaxed,
        Ordering::Relaxed,
    ) {
        Ok(_) => {
            // We set the guard to the init state, so we have exclusive control
            let state = unsafe { &mut *interrupt_state_ptr };
            *state = Some(interrupt_state);
            MAX_CORE_COUNT.store(max_core_count, Ordering::SeqCst);
            INTERRUPT_STATE_GUARD_PTR.store(interrupt_state_ptr, Ordering::SeqCst);

            return;
        }
        Err(guard_ptr) => guard_ptr as *const _,
    };

    while core::ptr::eq(guard_ptr, dummy_ptr) {
        spin_loop();
        guard_ptr = INTERRUPT_STATE_GUARD_PTR.load(Ordering::Relaxed) as *const _;
        assert_ne!(
            guard_ptr,
            null(),
            "INTERRUPT_STATE_PTR should never be reset to null"
        );
    }

    if core::ptr::eq(guard_ptr, interrupt_state_ptr) {
        assert_eq!(MAX_CORE_COUNT.load(Ordering::Relaxed), max_core_count, "init_interrupt_state must be called with the same argument for max_core_count every time");
        // Safety: guard_ptr points to final storage, so it will no longer be modified
        let state = unsafe { &*interrupt_state_ptr }
            .expect("guard_ptr points to final storage so it should be set");
        assert_eq!((state as *const dyn InterruptState).addr(), (interrupt_state as *const dyn InterruptState).addr(),
            "init_interrupt_state must be called with the same argument for interrupt_state every time");
    }
}

#[track_caller]
#[inline(always)]
fn interrupt_state() -> &'static dyn InterruptState {
    let guard_ptr = INTERRUPT_STATE_GUARD_PTR.load(Ordering::Relaxed) as *const _;
    let state_ptr = addr_of!(INTERRUPT_STATE);
    assert_eq!(guard_ptr, state_ptr, "init_interrupt_state not called");

    // Safety: guard_ptr points to final storage, so it will no longer be modified
    unsafe { &*state_ptr }.expect("interrupt state should be initialized based on guard")
}

impl CoreInfo for TestInterruptState {
    fn core_id(&self) -> shared::types::CoreId {
        interrupt_state().core_id()
    }

    fn is_bsp(&self) -> bool {
        interrupt_state().is_bsp()
    }

    fn is_initialized(&self) -> bool {
        interrupt_state().is_initialized()
    }

    fn instance() -> Self
    where
        Self: Sized,
    {
        TestInterruptState
    }

    fn max_core_count() -> u8
    where
        Self: Sized,
    {
        MAX_CORE_COUNT.load(Ordering::SeqCst)
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
