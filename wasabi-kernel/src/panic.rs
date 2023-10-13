//! panic handler implementation

use core::panic::PanicInfo;

use log::error;

use crate::{
    boot_info, cpu,
    graphics::{framebuffer::clear_frame_buffer, Color},
    locals,
    logger::LOGGER,
    serial_println,
};

/// Disables all other cores and interrupts.
///
/// # Safety:
///
/// This should only be called from panics
pub unsafe fn panic_disable_cores() {
    unsafe {
        // Safety: we are in a panic state, so anything relying on interrupts is
        // done for anyways
        locals!().disable_interrupts();

        // TODO disbale all other cores
    }
}

/// Ensures logger works during panic
///
/// This is done by recreating all resources that loggers depend on
/// or if that is not possible, disabling that logger
///
/// # Safety:
///
/// This should onl be called from panics, after all multicore and interrupts are
/// disabled.
pub unsafe fn recreate_logger() {
    // FIXME recreate loggers in panic
    //      without this logging during panic can deadlock
}

/// This function is called on panic.
#[panic_handler]
#[allow(unreachable_code)]
fn panic(info: &PanicInfo) -> ! {
    #[cfg(feature = "test")]
    crate::testing::panic::test_panic_handler(info);

    unsafe {
        // Safety: we are in a panic handler
        panic_disable_cores();

        // Safety: in panic handler after disable cores
        recreate_logger();
    };

    // Saftey: [LOGGER] is only writen to during the boot process.
    // Either we are in the boot process, in which case only we have access
    // or we aren't in which case everyone only reads
    if unsafe { &LOGGER }.is_none() {
        panic_no_logger(info);
    }

    if let Some(fb) = boot_info().framebuffer.as_mut() {
        clear_frame_buffer(fb, Color::PANIC);
    }

    error!("PANIC: {}", info);

    cpu::halt();
}

/// panic handler if we haven't initialized logging
fn panic_no_logger(info: &PanicInfo) -> ! {
    if let Some(fb) = boot_info().framebuffer.as_mut() {
        clear_frame_buffer(fb, Color::PANIC);
    }

    serial_println!("PANIC(no-logger): {}", info);

    cpu::halt();
}
