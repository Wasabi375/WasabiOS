//! panic handler implementation

use core::panic::PanicInfo;

use log::error;
use shared::lockcell::{LockCell, LockCellInternal, UnwrapLock};

use crate::{
    cpu,
    graphics::{
        fb::{
            startup::{take_boot_framebuffer, HARDWARE_FRAMEBUFFER_START_INFO},
            Framebuffer,
        },
        framebuffer, Canvas, Color,
    },
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
/// disabled and the framebuffer is useable
pub unsafe fn recreate_logger() {
    // FIXME recreate loggers in panic
    //      without this logging during panic can deadlock
}

/// Ensures frambuffer is accessible during panic
///
/// # Safety:
///
/// This should onl be called from panics, after all multicore and interrupts are
/// disabled. This function dose not require logging.
pub unsafe fn recreate_framebuffer() {
    if <UnwrapLock<_, _> as LockCellInternal<Framebuffer>>::is_unlocked(framebuffer()) {
        // if the framebuffer is unlocked we can just keep using the existing fb
        return;
    }
    let fb = match unsafe { HARDWARE_FRAMEBUFFER_START_INFO.take() } {
        Some((start, info)) => unsafe {
            // Safety:
            //  we are in a panic and therefor the memory will not be accessed any other way.
            Framebuffer::new_at_virt_addr(start, info)
        },
        None => unsafe {
            // Safety: during panic, and therefor unique access
            take_boot_framebuffer()
        }
        // NOTE: infinite panic loop here. Expect just for reasoning why this should be Some
        .expect("start/info static is not filled, therefor boot-framebuffer should exits")
        .into(),
    };
    // NOTE: this does not drop any existing fb, which is fine because we are in a panic
    // We do this just to recreate the lock
    framebuffer().lock_uninit().write(fb);
}

/// This function is called on panic.
#[panic_handler]
#[allow(unreachable_code)]
fn panic(info: &PanicInfo) -> ! {
    #[cfg(feature = "test")]
    crate::testing::panic::test_panic_handler(info);

    // Safety: we are in a panic handler
    unsafe {
        panic_disable_cores();

        recreate_framebuffer();

        recreate_logger();
    };

    framebuffer().lock().clear(Color::PINK);

    // Saftey: [LOGGER] is only writen to during the boot process.
    // Either we are in the boot process, in which case only we have access
    // or we aren't in which case everyone only reads
    if unsafe { &LOGGER }.is_none() {
        panic_no_logger(info);
    }

    error!("PANIC: {}", info);

    cpu::halt();
}

/// panic handler if we haven't initialized logging
fn panic_no_logger(info: &PanicInfo) -> ! {
    serial_println!("PANIC(no-logger): {}", info);

    cpu::halt();
}
