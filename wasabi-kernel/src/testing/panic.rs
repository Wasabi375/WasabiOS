//! Panic handler and recovery for tests

use alloc::boxed::Box;
use log::trace;
use logger::{debug_write, error_write, trace_write};
use uart_16550::SerialPort;

use crate::{locals, prelude::TicketLock, serial::SERIAL1, testing::qemu};
use core::{
    marker::PhantomData,
    panic::PanicInfo,
    sync::atomic::{AtomicU8, Ordering},
};
use shared::lockcell::{LockCell, LockCellInternal};

/// Function Type for custom panic handlers
type CustomPanicHandler = dyn (FnOnce(&PanicInfo) -> !) + Send + 'static;

/// holds the custom panic handler if it exists
static CUSTOM_PANIC: TicketLock<Option<Box<CustomPanicHandler>>> = TicketLock::new(None);

///
static PANIC_RECURSION_MARKER: AtomicU8 = AtomicU8::new(0);

/// panic handler used during tests
pub fn test_panic_handler(info: &PanicInfo) -> ! {
    let panic_recursion = PANIC_RECURSION_MARKER.fetch_add(1, Ordering::SeqCst);
    let write: &mut SerialPort = unsafe {
        // Safety: we are in a panic state, so anything relying on interrupts is
        // done for anyways
        locals!().disable_interrupts();

        // TODO disbale all other cores

        // Safety: interrupts and other cores disabled
        SERIAL1.get_mut()
    };

    if let Some(custom_panic_handler) = CUSTOM_PANIC.lock().take() {
        trace_write!(write, "Custom panic handler detected");
        if panic_recursion == 0 {
            custom_panic_handler(info);
            // unreachable
        } else {
            error_write!(write, "Custom Panic Handler paniced: ");
        }
    } else {
        if panic_recursion > 0 {
            error_write!(write, "main panic handler paniced");
            debug_write!(write, "panic recursion count: {}", panic_recursion);
        }
    }

    error_write!(write, "TEST PANIC: {info}");

    debug_write!(write, "panic! exit qemu");

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
