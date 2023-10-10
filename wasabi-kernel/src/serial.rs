//! Utiltities for accessing the serial port

use crate::prelude::{TicketLock, UnwrapTicketLock};
use shared::lockcell::LockCell;
use uart_16550::SerialPort;

/// address of the first IO port
const COM1_IO_PORT: u16 = 0x3F8;
/// address of the second IO port (used for testing)
const COM2_IO_PORT: u16 = 0x2F8;

// TODO serial ports should be optional.
// for logging I will have to rely on printing to Screen and

/// the first serial port
pub static SERIAL1: UnwrapTicketLock<SerialPort> =
    unsafe { UnwrapTicketLock::new_non_preemtable_uninit() };

/// the second serial port used for testing
pub static SERIAL2: TicketLock<Option<SerialPort>> = TicketLock::new_non_preemtable(None);

/// initializes the serialports that need to exist for the kernel to run.
///
/// This is COM1 for debug printing and also COM2 during testing
///
/// # Safety:
///
/// must only ever be called once at startup of the kernel boot process
pub unsafe fn init_serial_ports() {
    unsafe {
        // Safety: COM1_IO_PORT is a valid IO port
        SERIAL1
            .lock_uninit()
            .write(create_port(COM1_IO_PORT).unwrap());
        // Safety: COM2_IO_PORT is a valid IO port
        if let Some(com2) = create_port(COM2_IO_PORT).ok() {
            let _ = SERIAL2.lock().insert(com2);
        }
    }
}

/// Creates and initializes SerialPort
///
/// # Safety:
///
/// port must be a valid SerialPort address
unsafe fn create_port(port: u16) -> Result<SerialPort, ()> {
    unsafe {
        // Safety: port is valid
        SerialPort::try_create(port)
    }
}

#[doc(hidden)]
#[inline]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;

    let mut serial = SERIAL1.lock();

    serial.write_fmt(args).expect("Printing to serial failed");
}

/// Prints to the host through the serial interface.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*));
    };
}

/// Prints to the host through the serial interface, appending a newline.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}
