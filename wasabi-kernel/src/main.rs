//! A hobby os-kernel written in Rust

#![no_std]
#![no_main]
#![feature(
    error_in_core,
    ptr_alignment_type,
    abi_x86_interrupt,
    ascii_char,
    let_chains,
    allocator_api,
    result_flattening,
    maybe_uninit_uninit_array,
    maybe_uninit_array_assume_init
)]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod core_local;
pub mod cpu;
pub mod framebuffer;
pub mod logger;
pub mod mem;
pub mod panic;
pub mod prelude;
pub mod serial;
pub mod time;

#[allow(unused_imports)]
use log::{debug, info, trace, warn};
use shared::lockcell::LockCell;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    core_local::core_boot,
    cpu::{
        apic::{self, TimerConfig},
        cpuid, gdt, interrupts,
    },
};
use bootloader_api::{config::Mapping, BootInfo};
use core::ptr::null_mut;

extern crate alloc;

/// Contains the [BootInfo] provided by the Bootloader
/// TODO: this breaks rust uniquness guarantee
static mut BOOT_INFO: *mut BootInfo = null_mut();

/// returns the [BootInfo] provided by the bootloader
/// TODO: this breaks rust uniquness guarantee
pub fn boot_info() -> &'static mut BootInfo {
    unsafe { &mut *BOOT_INFO }
}

/// initializes the kernel.
fn init(boot_info: &'static mut BootInfo) {
    // Safety: TODO: boot info handling not really save, but it's fine for now
    // we are still single core and don't really care
    unsafe {
        BOOT_INFO = boot_info;
    }

    // Safety:
    // `init` is only called once per core and `core_boot`
    let core_id = unsafe { core_boot() };

    if core_id.is_bsp() {
        unsafe {
            time::calibrate_tsc();

            // Safety: bsp during `init`
            logger::init();

            // Safety: inherently unsafe and can crash, but if cpuid isn't supported
            // we will crash at some point in the future anyways, so we might as well
            // crash early
            cpuid::check_cpuid_usable();

            // Safety: bsp during `init` and locks and logging are working
            mem::init();
        }
    }

    // Safety:
    // this is called after `core_boot()` and we have initialized memory and logging
    unsafe {
        core_local::init(core_id);
    }

    gdt::init();
    interrupts::init();

    apic::init().unwrap();

    assert!(locals!().interrupts_enabled());
}

fn timer_int_handler(_vec: u8, _isf: InterruptStackFrame) -> Result<(), ()> {
    info!("hi");
    Ok(())
}

/// the main entry point for the kernel. Called by the bootloader.
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    init(boot_info);

    let startup_time = time::time_since_startup().to_millis();
    info!("tsc clock rate {}MHz", time::tsc_tickrate());
    info!("kernel boot took {:?} - {}", startup_time, startup_time);

    {
        let mut apic_guard = locals!().apic.lock();
        let apic = apic_guard.as_mut().unwrap();
        apic.timer().calibrate();
        info!("apic timer: {:#?}", apic.timer());

        apic.timer()
            .register_interrupt_handler(55, timer_int_handler)
            .unwrap();
        let apic_rate = apic.timer().rate_mhz() as u32;
        apic.timer().start(apic::TimerMode::Periodic(TimerConfig {
            divider: apic::TimerDivider::DivBy2,
            duration: apic_rate * 1_000_000 / 2,
        }));
    }

    // warn!("Causing fault");
    // let ptr = (0xdeadbeafu64 + 0x8000000000u64) as *mut u8;
    // unsafe {
    //     *ptr = 42;
    // }
    // info!("fault done");

    x86_64::instructions::interrupts::int3();

    info!("OS Done! cpu::halt()");
    cpu::halt();
}

/// configuration for the bootloader
const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let mut config = bootloader_api::BootloaderConfig::new_default();
    config.mappings.page_table_recursive = Some(Mapping::Dynamic);
    config
};
bootloader_api::entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);
