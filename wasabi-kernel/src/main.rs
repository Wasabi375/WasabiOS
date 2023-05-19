#![no_std]
#![no_main]

mod panic;
mod framebuffer;

use core::ptr::null_mut;

use bootloader_api::BootInfo;

static mut BOOT_INFO: *mut BootInfo = null_mut();
pub fn boot_info() -> &'static mut BootInfo {
    unsafe { &mut *BOOT_INFO }
}

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    unsafe {
        BOOT_INFO = boot_info;
    }
    loop {}
}

const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let mut config = bootloader_api::BootloaderConfig::new_default();
    config.kernel_stack_size = 100 * 1024; // 100 KiB
    config
};
bootloader_api::entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);
