//! A module containing logging and debug utilities

use core::ptr::{addr_of, addr_of_mut};

use log::{LevelFilter, Log, info};
use logger::{LogSetup, RefLogger, dispatch::TargetLogger};
use uart_16550::SerialPort;

use crate::{
    core_local::CoreInterruptState, prelude::UnwrapTicketLock, serial::SERIAL1, serial_println,
};

/// the max number of target loggers for the main dispatch logger
pub const MAX_LOG_DISPATCHES: usize = 2;

/// number of module filers allowed for the logger
pub const MAX_LEVEL_FILTERS: usize = 100;

/// number of module renames allowed for the logger
pub const MAX_RENAME_MAPPINGS: usize = 100;

/// See [logger::DispatchLogger]
pub type DispatchLogger<'a, const N: usize, const L: usize> =
    logger::DispatchLogger<'a, CoreInterruptState, N, L>;

/// the static logger used by the [log::log] macro
///
/// After [init] is called, it can be assumed that this is Some.
///
/// # Safety
/// this should not be modified outside of panics and [init].
/// Accessing the [DispatchLogger] via shared ref is therefor safe.
#[inline]
pub fn static_logger()
-> Option<&'static DispatchLogger<'static, MAX_LOG_DISPATCHES, MAX_LEVEL_FILTERS>> {
    unsafe { &*addr_of_mut!(LOGGER) }.as_ref()
}
static mut LOGGER: Option<DispatchLogger<'static, MAX_LOG_DISPATCHES, MAX_LEVEL_FILTERS>> = None;

/// the static serial logger.
static mut SERIAL_LOGGER: Option<
    RefLogger<
        'static,
        SerialPort,
        UnwrapTicketLock<SerialPort>,
        CoreInterruptState,
        0,
        MAX_RENAME_MAPPINGS,
    >,
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
/// # Safety
///
/// must only ever be called once at the start of the kernel boot proces and after
/// [SERIAL1] is initialized
pub unsafe fn init() {
    assert!(
        MAX_LEVEL_FILTERS + MAX_RENAME_MAPPINGS <= 253,
        "MAX_LEVEL_FILTERS + MAX_RENAME_MAPPINGS must be <= 253 or else \
            StaticLogger doesn't fit in 4KiB which causes tripple fault on boot"
    );

    // TODO reduce the stack size requirements for logging

    let mut dispatch_logger = DispatchLogger::new()
        .with_level(LevelFilter::Info)
        // comment to move ; to separate line - easy uncomment of module log levels
        ;
    #[cfg(not(feature = "test"))]
    #[allow(dead_code)]
    {
        dispatch_logger = dispatch_logger
            // .with_module_level("wasabi_kernel::task", LevelFilter::Trace)
            // .with_module_level("GlobalAlloc", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::cpu", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::cpu::acpi", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::cpu::apic", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::core_local", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::mem", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::mem::kernel_heap", LevelFilter::Info)
            // .with_module_level("GlobalAlloc", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::graphics", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::cpu::apic::ap_startup", LevelFilter::Debug)
            // .with_module_level("wasabi_kernel::panic", LevelFilter::Trace);
            // .with_module_level("wasabi_kernel::pci", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::pci::nvme", LevelFilter::Trace)
            // .with_module_level("wasabi_kernel::pci::nvme::queue", LevelFilter::Debug)
            // .with_module_level("wasabi_kernel::crossbeam_epoch", LevelFilter::Trace)
            // comment to move ; to separate line - easy uncomment of module log levels
            ;
    }

    #[cfg(feature = "test")]
    {
        // adjust log levels for tests
        dispatch_logger = dispatch_logger
            .with_level(LevelFilter::Warn)
            .with_module_level("wasabi_test", LevelFilter::Info)
            // .with_module_level("wasabi_kernel::mem::page_table::test", LevelFilter::Info)
            // .with_module_level("wasabi_kernel::pci::nvme", LevelFilter::Debug)
            // .with_module_level("wfs::mem_tree", LevelFilter::Trace)
            // test tests
            // .with_module_level("wasabi_test", LevelFilter::Trace)
            // .with_module_level("test_serial", LevelFilter::Trace)
            // comment to move ; to separate line - easy uncomment of module log levels
            ;
    }

    let mut serial_logger = RefLogger::new(&SERIAL1);
    setup_logger_module_rename(&mut serial_logger);
    serial_logger.init();

    // Safety: this is fine, since we are in the kernel boot and this is only
    // called once, meaning we fullfill rust mutability guarantees
    unsafe {
        SERIAL_LOGGER = Some(serial_logger);
        dispatch_logger.with_logger(TargetLogger::new_primary(
            "serial",
            (&*addr_of_mut!(SERIAL_LOGGER)).as_ref().unwrap(),
        ));
    }

    dispatch_logger.init();
    log::set_max_level(LevelFilter::Trace);

    // Safety: see above
    if unsafe {
        LOGGER = Some(dispatch_logger);

        set_global_logger(static_logger().unwrap_unchecked());

        log::set_logger(&*addr_of!(GLOBAL_LOGGER))
    }
    .is_err()
    {
        serial_println!("!!! Failed to init logger !!!!");

        // Safety: this is fine, since we are in the kernel boot and this is only
        // called once, meaning we ensure rust mutability guarantees
        panic!();
    }

    info!("Static Logger initialized to Serial Port 1");
}

static mut GLOBAL_LOGGER: GlobalLogger = GlobalLogger { logger: None };

/// Set the logger as the global logger
///
/// # Safety
///
/// must only be called while neither `logger` or the current global logger
/// is accessed
pub unsafe fn set_global_logger(logger: &'static dyn Log) {
    unsafe {
        GLOBAL_LOGGER.logger = Some(logger);
    }
}

struct GlobalLogger {
    logger: Option<&'static dyn Log>,
}

impl Log for GlobalLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Safety: see set_global_logger
        unsafe { self.logger.unwrap_unchecked() }.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        // Safety: see set_global_logger
        unsafe { self.logger.unwrap_unchecked() }.log(record)
    }

    fn flush(&self) {
        // Safety: see set_global_logger
        unsafe { self.logger.unwrap_unchecked() }.flush()
    }
}
