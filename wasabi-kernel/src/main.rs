//! Main entry for hobby os-kernel written in Rust
#![no_std]
#![no_main]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

#[macro_use]
extern crate wasabi_kernel;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use alloc::{format, string::String};
use bootloader_api::BootInfo;
use wasabi_kernel::{
    KernelConfig, bootloader_config_common, default_kernel_config,
    task::TaskDefinition,
    time::{self},
};

#[cfg(feature = "mem-stats")]
use wasabi_kernel::mem::{
    frame_allocator::FrameAllocator, kernel_heap::KernelHeap, page_allocator::PageAllocator,
    page_table::PageTable,
};
use x86_64::registers::rflags::{self, RFlags};

/// the main entry point for the kernel. Called by the bootloader.
fn kernel_main() {
    if locals!().is_bsp() {
        let startup_time = time::time_since_startup().to_millis();
        info!("tsc clock rate {}MHz", time::tsc_tickrate());
        warn!("kernel boot took {:?} - {}", startup_time, startup_time);
    }

    if locals!().is_bsp() {
        // wasabi_kernel::pci::pci_experiment();
    }
    context_switch_experiment();

    #[cfg(feature = "mem-stats")]
    if locals!().is_bsp() {
        // TODO figure out how I can do this at the end of the kernel, with a running Task System
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

    info!("OS core cpu task is done!\t");
}

fn context_switch_experiment() {
    assert!(interrupts_enabled());
    let task_system = &locals!().task_system;
    // // Safety: tasks do not share stack references
    unsafe {
        let foo = 5u64;
        let bar = 5u64;
        task_system
            .launch_task(TaskDefinition::with_name(
                move || info!("foobar: {}", foo + bar),
                "foobar",
            ))
            .unwrap();
        task_system
            .launch_task(TaskDefinition::with_name(count_task, "count"))
            .unwrap();

        for i in 0..25 {
            task_system
                .launch_task(TaskDefinition::with_name(
                    calc_pi,
                    String::leak(format!("pi {i}")),
                ))
                .unwrap();
            task_system
                .launch_task(TaskDefinition::with_name(
                    find_primes,
                    String::leak(format!("primes {i}")),
                ))
                .unwrap();
        }
    }

    info!("task count: {}", task_system.get_task_count());
    task_system.start().expect("Timer not in use");
}

fn interrupts_enabled() -> bool {
    let flags = rflags::read();
    flags.contains(RFlags::INTERRUPT_FLAG)
}

#[allow(unused)]
fn count_task() {
    info!("in count task");
    for i in 0..100 {
        info!("{i}");
    }
}

#[allow(unused)]
fn find_primes() {
    fn is_prime(x: u64) -> bool {
        assert!(interrupts_enabled());
        if x == 2 {
            return true;
        }
        if x % 2 == 0 {
            return false;
        }
        let mut factor = 3;
        while factor < x / 2 {
            if x % factor == 0 {
                return false;
            }
            factor += 2;
        }
        true
    }

    let mut last_prime = 1;
    for i in 2.. {
        if is_prime(i) {
            if last_prime + 2 == i {
                info!("{last_prime} and {i} are prime twins");
            } else {
                info!("{i} is prime");
            }
            last_prime = i;
        } else {
        }
    }
}

#[allow(unused)]
fn calc_pi() {
    let mut sum = 0.0;
    let mut sign = 1;
    for k in 0u64.. {
        let term = sign as f64 / (2.0 * k as f64 + 1.0);
        sign *= -1;
        sum += term;

        if k % 100_000 == 0 {
            info!("pi({}e5) = {}", k / 100_000, 4.0 * sum);
        }
    }
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
