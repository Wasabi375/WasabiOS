#![no_std]
#![no_main]
#![feature(
    error_in_core,
    ptr_alignment_type,
    abi_x86_interrupt,
    ascii_char,
    let_chains,
    allocator_api
)]
#![allow(dead_code)]

pub mod cpu;
pub mod debug_logger;
pub mod framebuffer;
pub mod mem;
pub mod panic;
pub mod serial;

#[allow(unused_imports)]
use log::{debug, info, trace, warn};

use crate::cpu::{cpuid, gdt, interrupts};
use alloc::boxed::Box;
use bootloader_api::{config::Mapping, BootInfo};
use core::ptr::null_mut;

extern crate alloc;

// FIXME: this breaks rust uniquness guarantee
static mut BOOT_INFO: *mut BootInfo = null_mut();
pub fn boot_info() -> &'static mut BootInfo {
    unsafe { &mut *BOOT_INFO }
}

fn init(boot_info: &'static mut BootInfo) {
    unsafe {
        BOOT_INFO = boot_info;
    }
    debug_logger::init();

    trace!("{boot_info:#?}");

    cpuid::check_cpuid_usable();

    mem::init();

    {
        let _ = Box::new(5u8);
        let _ = Box::new(5u16);
        let _ = Box::new(5u32);
        let _ = Box::new(5u64);
        let _ = Box::new(5u128);
        let _ = Box::new([5u8; 256]);
        let _ = Box::new([5u8; 512]);
    }
    {
        for _ in 0..10000 {
            let a = Box::new(6u128);
            let b = Box::new(7u128);
            let c = Box::new(2u128);
        }
    }

    // apic::init();

    gdt::init();
    interrupts::init();
}

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    init(boot_info);

    // warn!("Causing fault");
    // let ptr = (0xdeadbeafu64 + 0x8000000000u64) as *mut u8;
    // unsafe {
    //     *ptr = 42;
    // }
    // info!("fault done");

    // x86_64::instructions::interrupts::int3();

    info!("OS Done! cpu::halt()");
    cpu::halt();
}

const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let mut config = bootloader_api::BootloaderConfig::new_default();
    config.mappings.page_table_recursive = Some(Mapping::Dynamic);
    config
};
bootloader_api::entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);
