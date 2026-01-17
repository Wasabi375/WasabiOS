use core::{arch::asm, panic::PanicInfo};

/// This function is called on panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    panic_impl(info);
}

fn panic_impl(_info: &PanicInfo) -> ! {
    // TODO panic in kernel_module
    loop {
        unsafe {
            asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
