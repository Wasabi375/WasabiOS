use crate::prelude::TicketLock;
use lazy_static::lazy_static;
use uart_16550::SerialPort;

const COM1_IO_PORT: u16 = 0x3F8;

lazy_static! {
    pub static ref SERIAL1: TicketLock<SerialPort> = {
        // Safety: COM1_IO_PORT is a valid Serial port
        let mut port = unsafe { SerialPort::new(COM1_IO_PORT) };
        port.init();
        TicketLock::new(port)
    };
}

#[doc(hidden)]
#[inline]
pub fn _print(args: core::fmt::Arguments) {
    use core::fmt::Write;
    use shared::lockcell::LockCell;

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
