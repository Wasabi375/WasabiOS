//! panic handler implementation

use core::panic::PanicInfo;
use lazy_static::__Deref;
use log::error;

use crate::{
    boot_info, cpu,
    debug_logger::LOGGER,
    framebuffer::{clear_frame_buffer, Color},
    serial::SERIAL1,
    serial_println,
};
use shared::lockcell::LockCellInternal;

/// This function is called on panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        // TODO try with timeout first, before clobbering lock
        SERIAL1.deref().force_unlock();
    }

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
