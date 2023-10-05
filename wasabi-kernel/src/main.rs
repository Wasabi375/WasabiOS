//! Main entry for hobby os-kernel written in Rust
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

#[macro_use]
extern crate wasabi_kernel;

#[allow(unused_imports)]
use log::{debug, info, trace, warn};

use bootloader_api::BootInfo;
use shared::lockcell::LockCell;
use wasabi_kernel::{
    bootloader_config_common,
    cpu::{
        self,
        apic::{self, TimerConfig},
    },
    init,
    serial::SERIAL2,
    time::{self, sleep_tsc},
};
use x86_64::structures::idt::InterruptStackFrame;

fn timer_int_handler(_vec: u8, _isf: InterruptStackFrame) -> Result<(), ()> {
    trace!("hi");
    Ok(())
}

/// the main entry point for the kernel. Called by the bootloader.
fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    init(boot_info);

    let startup_time = time::time_since_startup().to_millis();
    info!("tsc clock rate {}MHz", time::tsc_tickrate());
    info!("kernel boot took {:?} - {}", startup_time, startup_time);

    if SERIAL2.lock().is_some() {
        info!("SERIAL2 found!");
    } else {
        info!("SERIAL2 not found");
    }

    {
        let mut apic_guard = locals!().apic.lock();
        let apic = apic_guard.as_mut().as_mut().unwrap();
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

    if let Some(serial2) = SERIAL2.lock().as_mut() {
        let mut count: u8 = 0;
        loop {
            sleep_tsc(time::Duration::Millis(10));
            serial2.send_raw(count);
            if count == u8::MAX {
                count = 0;
            } else {
                count += 1;
            }
        }
    }

    info!("OS Done! cpu::halt()");
    cpu::halt();
}

/// configuration for the bootloader
const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let config = bootloader_api::BootloaderConfig::new_default();
    bootloader_config_common(config)
};
bootloader_api::entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);
