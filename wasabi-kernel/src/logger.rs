//! A module containing logging and debug utilities

use log::{info, LevelFilter};
use logger::{dispatch::TargetLogger, LogSetup, RefLogger};
use uart_16550::SerialPort;

use crate::{
    core_local::CoreInterruptState, prelude::UnwrapTicketLock, serial::SERIAL1, serial_println,
};

/// the max number of target loggers for the main dispatch logger
const MAX_LOG_DISPATCHES: usize = 2;

/// number of module filers allowed for the logger
const MAX_LEVEL_FILTERS: usize = 100;

/// number of module renames allowed for the logger
const MAX_RENAME_MAPPINGS: usize = 100;

/// See [logger::DispatchLogger]
pub type DispatchLogger<'a, const N: usize, const L: usize> =
    logger::DispatchLogger<'a, CoreInterruptState, N, L>;

/// the static logger used by the [log::log] macro
///
/// After [init] is called, it can be assumed that this is Some.
///
/// # Safety:
/// this should not be modified outside of panics and [init].
/// Accessing the [DispatchLogger] via shared ref is safe.
pub static mut LOGGER: Option<DispatchLogger<'static, MAX_LOG_DISPATCHES, MAX_LEVEL_FILTERS>> =
    None;

/// the static serial logger.
static mut SERIAL_LOGGER: Option<
    RefLogger<'static, SerialPort, UnwrapTicketLock<SerialPort>, 0, MAX_RENAME_MAPPINGS>,
> = None;

/// setup module renames for any static logger
pub fn setup_logger_module_rename<L>(logger: &mut L)
where
    L: LogSetup,
{
    logger
        .with_module_rename("wasabi_kernel::cpu::interrupts", "::cpu::int")
        .with_module_rename("wasabi_kernel::", "::")
        .with_module_rename("wasabi_kernel", "::");
}

/// initializes the logger piping all [log::log] calls into the first serial port.
///
/// # Safety:
///
/// must only ever be called once at the start of the kernel boot proces and after
/// [SERIAL1] is initialized
pub unsafe fn init() {
    assert!(
        MAX_LEVEL_FILTERS + MAX_RENAME_MAPPINGS <= 253,
        "MAX_LEVEL_FILTERS + MAX_RENAME_MAPPINGS must be <= 253 or else \
            StaticLogger doesn't fit in 4KiB which causes tripple fault on boot"
    );

    let mut dispatch_logger = DispatchLogger::new()
        .with_level(LevelFilter::Debug)
        // .with_level(LevelFilter::Info)
        // .with_level(LevelFilter::Trace)
        // .with_module_level("wasabi_kernel", LevelFilter::Trace)
        .with_module_level("wasabi_kernel::cpu", LevelFilter::Trace)
        .with_module_level("wasabi_kernel::core_local", LevelFilter::Trace)
        //.with_module_level("wasabi_kernel::mem", LevelFilter::Trace)
        // .with_module_level("GlobalAlloc", LevelFilter::Trace)
        // .with_module_level("wasabi_kernel::graphics", LevelFilter::Trace)
        // comment to move ; to separate line - easy uncomment of module log levels
        ;

    let mut serial_logger = RefLogger::new(&SERIAL1);
    setup_logger_module_rename(&mut serial_logger);
    serial_logger.init();

    // Safety: this is fine, since we are in the kernel boot and this is only
    // called once, meaning we fullfill rust mutability guarantees
    unsafe {
        SERIAL_LOGGER = Some(serial_logger);
        dispatch_logger.with_logger(TargetLogger::new_primary(
            "serial",
            SERIAL_LOGGER.as_ref().unwrap(),
        ));
    }

    dispatch_logger.init();
    log::set_max_level(LevelFilter::Trace);

    // Safety: see above
    if unsafe {
        LOGGER = Some(dispatch_logger);

        let logger = LOGGER.as_mut().unwrap_unchecked();
        logger.set_globally()
    }
    .is_err()
    {
        serial_println!("!!! Failed to init logger !!!!");

        // Safety: this is fine, since we are in the kernel boot and this is only
        // called once, meaning we ensure rust mutability guarantees
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
