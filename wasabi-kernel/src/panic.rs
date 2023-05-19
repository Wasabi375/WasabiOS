use core::panic::PanicInfo;

use crate::{boot_info, framebuffer::clear_frame_buffer};

/// This function is called on panic.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    if let Some(fb) = boot_info().framebuffer.as_mut() {
        clear_frame_buffer(fb, 255, 0, 255);
    }
    loop {}
}
