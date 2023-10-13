//! Panic handler and recovery for tests

use alloc::boxed::Box;
use log::{debug, error, trace};

use crate::{
    panic::{panic_disable_cores, recreate_logger},
    prelude::TicketLock,
    testing::qemu,
};
use core::{
    marker::PhantomData,
    panic::PanicInfo,
    sync::atomic::{AtomicU8, Ordering},
};
use shared::lockcell::LockCell;

/// Function Type for custom panic handlers
type CustomPanicHandler = dyn (FnOnce(&PanicInfo) -> !) + Send + 'static;

/// holds the custom panic handler if it exists
static CUSTOM_PANIC: TicketLock<Option<Box<CustomPanicHandler>>> = TicketLock::new(None);

/// marks that we are currently in a panic if this is not 0
static PANIC_RECURSION_MARKER: AtomicU8 = AtomicU8::new(0);

/// panic handler used during tests
pub fn test_panic_handler(info: &PanicInfo) -> ! {
    let panic_recursion = PANIC_RECURSION_MARKER.fetch_add(1, Ordering::SeqCst);

    unsafe {
        // Safety: we are in a panic handler
        panic_disable_cores();

        // Safety: in panic handler after disable cores
        recreate_logger();
    };

    if let Some(custom_panic_handler) = CUSTOM_PANIC.lock().take() {
        trace!("Custom panic handler detected");
        if panic_recursion == 0 {
            custom_panic_handler(info);
            // unreachable
        } else {
            error!("Custom Panic Handler paniced: ");
        }
    } else {
        if panic_recursion > 0 {
            error!("main panic handler paniced");
            debug!("panic recursion count: {}", panic_recursion);
        }
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
        let mut custom_panic = CUSTOM_PANIC.lock();
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
    let mut custom_panic = CUSTOM_PANIC.lock();
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
