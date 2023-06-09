//! panic handler implementation

use core::panic::PanicInfo;
use lazy_static::__Deref;
use log::error;

use crate::{
    boot_info, cpu,
    framebuffer::{clear_frame_buffer, Color},
    locals,
    logger::LOGGER,
    serial::SERIAL1,
    serial_println,
};
use shared::lockcell::LockCellInternal;

/// This function is called on panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        // Safety: we are in a panic state, so anything relying on interrupts is
        // done for anyways
        locals!().disable_interrupts();

        // TODO: kill other cores before clobbering the lock
        // TODO try with timeout first, before clobbering lock
        // Safety: we panic, and no longer care. We just want to log
        SERIAL1.deref().force_unlock();
    }

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
