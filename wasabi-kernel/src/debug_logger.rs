//! A module containing logging and debug utilities
use lazy_static::__Deref;
use log::{info, LevelFilter};
use logger::StaticLogger;
use uart_16550::SerialPort;

use crate::{prelude::TicketLock, serial::SERIAL1, serial_println};

/// the static logger used by the [log::log] macro
pub static mut LOGGER: Option<StaticLogger<'static, SerialPort, TicketLock<SerialPort>>> = None;

/// initializes the logger piping all [log::log] calls into the first serial port.
pub fn init() {
    let logger = StaticLogger::new(SERIAL1.deref())
        .with_level(LevelFilter::Debug)
        // .with_level(LevelFilter::Trace)
        .with_module_level("wasabi_kernel::cpu", LevelFilter::Trace)
        .with_module_level("wasabi_kernel::core_local", LevelFilter::Trace)
        // .with_module_level("wasabi_kernel::mem", LevelFilter::Trace)
        // .with_module_level("GlobalAlloc", LevelFilter::Trace)
        // comment to move ; to separate line - easy uncomment of module log levels
        ;
    if unsafe {
        LOGGER = Some(logger);

        LOGGER.as_mut().unwrap_unchecked().init()
    }
    .is_err()
    {
        serial_println!("!!! Failed to init logger !!!!");
        unsafe {
            LOGGER = None;
        }
        panic!();
    }

    info!("Static Logger initialized to Serial Port 1");
}

/// A macro logging and returning the result of any expression.
/// The result of the expression is logged using the [log::debug] macro.
///
/// ```
/// assert_eq!(5, dbg!(5)); // also calls log::debug(5)
/// ```
#[allow(unused_macros)]
#[macro_export]
macro_rules! dbg {
    ($v:expr) => {{
        let value = $v;
        log::debug!("{value:?}");
        value
    }};
}

/// Same as [todo!] but only calls a [log::warn] instead of [panic].
#[allow(unused_macros)]
#[macro_export]
macro_rules! todo_warn {
    () => {
        log::warn!("not yet implemented")
    };
    ($($arg:tt)+) => {
        log::warn!("not yet implemented: {}", $crate::format_args!($($arg)+))
    };
}

/// Same as [todo!] but only calls a [log::error] instead of [panic].
#[allow(unused_macros)]
#[macro_export]
macro_rules! todo_error {
    () => {
        log::error!("not yet implemented")
    };
    ($($arg:tt)+) => {
        log::error!("not yet implemented: {}", $crate::format_args!($($arg)+))
    };
}
