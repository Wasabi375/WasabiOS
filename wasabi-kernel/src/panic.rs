//! panic handler implementation

use core::panic::PanicInfo;
use logger::error_write;
use shared::lockcell::LockCellInternal;

use crate::{
    boot_info, cpu,
    framebuffer::{clear_frame_buffer, Color},
    locals,
    logger::LOGGER,
    serial::SERIAL1,
    serial_println,
};

/// This function is called on panic.
#[panic_handler]
#[allow(unreachable_code)]
fn panic(info: &PanicInfo) -> ! {
    #[cfg(feature = "test")]
    crate::testing::panic::test_panic_handler(info);

    let write = unsafe {
        // Safety: we are in a panic state, so anything relying on interrupts is
        // done for anyways
        locals!().disable_interrupts();

        // TODO disbale all other cores

        // Safety: interrupts and other cores disabled
        SERIAL1.get_mut()
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

    error_write!(write, "PANIC: {}", info);

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
