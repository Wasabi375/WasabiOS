#![no_std]
#![no_main]
#![feature(error_in_core, ptr_alignment_type)]
#![feature(abi_x86_interrupt)]
#![allow(dead_code)]

pub mod cpu;
pub mod debug;
pub mod framebuffer;
pub mod mem;
pub mod panic;
pub mod serial;

use core::ptr::null_mut;

use bootloader_api::BootInfo;
use log::info;

use crate::cpu::{gdt, interrupts};

static mut BOOT_INFO: *mut BootInfo = null_mut();
pub fn boot_info() -> &'static mut BootInfo {
    unsafe { &mut *BOOT_INFO }
}

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    unsafe {
        BOOT_INFO = boot_info;
    }

    debug::init();

    gdt::init();
    interrupts::init();

    x86_64::instructions::interrupts::int3();

    info!("OS Done! cpu::halt()");
    cpu::halt();
}

const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let config = bootloader_api::BootloaderConfig::new_default();
    config
};
bootloader_api::entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);
