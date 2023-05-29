use log::{info, LevelFilter};
use logger::StaticLogger;
use uart_16550::SerialPort;

use crate::{
    boot_info,
    framebuffer::{clear_frame_buffer, Color},
    serial::SERIAL1,
    serial_println,
};

pub static mut LOGGER: Option<StaticLogger<'static, SerialPort>> = None;

pub fn init() {
    let logger = StaticLogger::new(&SERIAL1)
        .with_level(LevelFilter::Debug)
        // .with_module_level("wasabi_kernel::cpu", LevelFilter::Trace)
        .with_module_level("wasabi_kernel::mem", LevelFilter::Trace)
        // .with_module_level("GlobalAlloc", LevelFilter::Trace)
        // .with_level(LevelFilter::Error)
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

#[allow(unused_macros)]
#[macro_export]
macro_rules! dbg {
    ($v:expr) => {{
        let value = $v;
        log::debug!("{value:?}");
        value
    }};
}

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

pub fn fill_screen(c: Color) {
    if let Some(fb) = boot_info().framebuffer.as_mut() {
        clear_frame_buffer(fb, c);
    }
}

pub fn debug_clear_screen() {
    fill_screen(Color::BLACK);
}
