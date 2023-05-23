use core::panic::PanicInfo;
use log::error;

use crate::debug::LOGGER;
use crate::framebuffer::{clear_frame_buffer, Color};
use crate::{boot_info, cpu, serial_println};

/// This function is called on panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // TODO clobber serial lock if necessary
    if unsafe { &LOGGER }.is_none() {
        panic_no_logger(info);
    }
    error!("PANIC: {}", info);

    if let Some(fb) = boot_info().framebuffer.as_mut() {
        clear_frame_buffer(fb, Color::PANIC);
    }

    cpu::halt();
}

fn panic_no_logger(info: &PanicInfo) -> ! {
    serial_println!("PANIC(no-logger): {}", info);

    if let Some(fb) = boot_info().framebuffer.as_mut() {
        clear_frame_buffer(fb, Color::PANIC);
    }

    cpu::halt();
}
