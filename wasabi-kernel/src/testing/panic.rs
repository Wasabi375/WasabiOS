//! Panic handler and recovery for tests

#[allow(unused_imports)]
use log::{debug, error, info, warn};
#[allow(unused_imports)]
use logger::{debug_write, error_write, info_write};
use uart_16550::SerialPort;

use crate::{locals, serial::SERIAL1, testing::qemu};
use core::panic::PanicInfo;
use shared::lockcell::LockCellInternal;

/// panic handler used during tests
pub fn test_panic_handler(info: &PanicInfo) -> ! {
    let write: &mut SerialPort = unsafe {
        // Safety: we are in a panic state, so anything relying on interrupts is
        // done for anyways
        locals!().disable_interrupts();

        // TODO disbale all other cores

        // Safety: interrupts and other cores disabled
        SERIAL1.get_mut()
    };

    error_write!(write, "TEST PANIC: {info}");

    debug_write!(write, "panic! exit qemu");

    qemu::exit(qemu::ExitCode::Error);
}
