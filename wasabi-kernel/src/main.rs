//! Main entry for hobby os-kernel written in Rust
#![no_std]
#![no_main]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

#[macro_use]
extern crate wasabi_kernel;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use bootloader_api::BootInfo;
use shared::sync::lockcell::LockCell;
use wasabi_kernel::{
    bootloader_config_common,
    cpu::{self, apic::timer::TimerConfig, interrupts::InterruptVector},
    default_kernel_config,
    kernel_info::KernelInfo,
    pci, time, KernelConfig,
};
use x86_64::structures::idt::InterruptStackFrame;

#[cfg(feature = "mem-stats")]
use wasabi_kernel::mem::{
    frame_allocator::FrameAllocator, kernel_heap::KernelHeap, page_allocator::PageAllocator,
    page_table::PageTable,
};

fn timer_int_handler(_vec: InterruptVector, _isf: InterruptStackFrame) -> Result<(), ()> {
    info!(target: "Timer", "tick");
    Ok(())
}

/// the main entry point for the kernel. Called by the bootloader.
fn kernel_main() -> ! {
    if locals!().is_bsp() {
        let startup_time = time::time_since_startup().to_millis();
        info!("tsc clock rate {}MHz", time::tsc_tickrate());
        warn!("kernel boot took {:?} - {}", startup_time, startup_time);
    }

    if locals!().is_bsp() {
        // TODO temp
        info!("rsdp at: {:?}", KernelInfo::get().boot_info.rsdp_addr);
        pci::pci_experiment();
    }

    // start_timer();

    //sleep_tsc(Duration::Seconds(5));
    //stop_timer();

    #[cfg(feature = "mem-stats")]
    if locals!().is_bsp() {
        let level = log::Level::Info;
        KernelHeap::get().lock().stats().log(level);
        PageAllocator::get_for_kernel()
            .lock()
            .stats()
            .log(level, Some("pages"));
        FrameAllocator::get_for_kernel()
            .lock()
            .stats()
            .log(level, Some("frames"));
        PageTable::get_for_kernel().lock().stats().log(level);
    }

    info!("OS Done!\tcpu::halt()");
    cpu::halt();
}

#[allow(dead_code)]
fn stop_timer() {
    let mut apic = locals!().apic.lock();
    let mut timer = apic.timer();

    timer.stop();
    info!("timer stopped!");
}

#[allow(dead_code)]
fn start_timer() {
    use cpu::apic::timer::{TimerDivider, TimerMode};

    let mut apic = locals!().apic.lock();
    let mut timer = apic.timer();

    timer
        .register_interrupt_handler(InterruptVector::Timer, timer_int_handler)
        .unwrap();
    let apic_rate = timer.rate_mhz() as u32;
    timer.start(TimerMode::Periodic(TimerConfig {
        divider: TimerDivider::DivBy2,
        duration: apic_rate * 1_000_000 / 2,
    }));
    trace!("apic timer: {:#?}", timer);
}

/// configuration for the bootloader
const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let config = bootloader_api::BootloaderConfig::new_default();
    bootloader_config_common(config)
};
const KERNEL_CONFIG: KernelConfig = {
    let mut config = default_kernel_config();
    config.start_aps = true;
    config
};
wasabi_kernel::entry_point!(
    kernel_main,
    boot_config = &BOOTLOADER_CONFIG,
    kernel_config = KERNEL_CONFIG
);
