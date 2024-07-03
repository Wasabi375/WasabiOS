//! Panic handler and recovery for tests

use alloc::boxed::Box;
use log::{debug, error, trace};

use crate::{test_locals, testing::qemu};
use core::{marker::PhantomData, panic::PanicInfo};
use shared::sync::lockcell::LockCell;

/// Function Type for custom panic handlers
pub type CustomPanicHandler = dyn (FnOnce(&PanicInfo) -> !) + Send + 'static;

/// panic handler used during tests
///
/// # Safety:
///
/// requires all other cores to be disabled and that the logger and framebuffer are working
pub unsafe fn test_panic_handler(info: &PanicInfo) -> ! {
    if let Some(custom_panic_handler) = test_locals!().custom_panic.lock().take() {
        trace!("Custom panic handler detected");
        custom_panic_handler(info);
        // unreachable
    }
    error!("TEST PANIC: {info}");

    debug!("panic! exit qemu");

    qemu::exit(qemu::ExitCode::Error);
    // unreachable
}

/// A guard structure that clears out the custom panic handler when dropped
pub struct CustomPanicHandlerGuard {
    _private_construct: PhantomData<()>,
}

impl Drop for CustomPanicHandlerGuard {
    fn drop(&mut self) {
        let mut custom_panic = test_locals!().custom_panic.lock();
        let handler = custom_panic.take();
        assert!(
            handler.is_some(),
            "Expected to drop custom panic handler but none existed"
        );
        trace!("drop CustomPanicHandlerGuard");
    }
}

/// Sets a custom panic handler.
///
/// Returns a guard structure that clears the handler when dropped.  This
/// function will panic if called multiple times, without first dropping the
/// guard.
pub fn use_custom_panic_handler<F>(panic_handler: F) -> CustomPanicHandlerGuard
where
    F: FnOnce(&PanicInfo) -> !,
    F: Send + 'static,
{
    let mut custom_panic = test_locals!().custom_panic.lock();
    assert!(
        custom_panic.is_none(),
        "There already exists a custom panic handler"
    );

    let _ = custom_panic.insert(Box::new(panic_handler));
    trace!("custom panic handler installed");

    CustomPanicHandlerGuard {
        _private_construct: PhantomData,
    }
}
