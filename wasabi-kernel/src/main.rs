//! Main entry for hobby os-kernel written in Rust
#![no_std]
#![no_main]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

#[macro_use]
extern crate wasabi_kernel;

use core::arch::asm;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use bootloader_api::BootInfo;
use wasabi_kernel::{
    KernelConfig, bootloader_config_common,
    cpu::interrupts::InterruptVector,
    default_kernel_config,
    task::{TaskDefinition, TaskSystem},
    time::{self},
};

#[cfg(feature = "mem-stats")]
use wasabi_kernel::mem::{
    frame_allocator::FrameAllocator, kernel_heap::KernelHeap, page_allocator::PageAllocator,
    page_table::PageTable,
};

/// the main entry point for the kernel. Called by the bootloader.
fn kernel_main() -> ! {
    if locals!().is_bsp() {
        let startup_time = time::time_since_startup().to_millis();
        info!("tsc clock rate {}MHz", time::tsc_tickrate());
        warn!("kernel boot took {:?} - {}", startup_time, startup_time);
    }

    if locals!().is_bsp() {
        // wasabi_kernel::pci::pci_experiment();
        context_switch_experiment();
    }

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

    info!("OS core cpu task is done!\t");

    // TODO move into lib. Change kernel_main to return unit
    unsafe {
        // Safety: cpu-core task has a static lifetime stack
        TaskSystem::terminate_task();
    }
}

fn context_switch_experiment() {
    unsafe {
        let foo = 5u64;
        let bar = 5u64;
        // Safety: tasks do not share stack references
        TaskSystem::launch_task(TaskDefinition::new(move || info!("foobar: {}", foo + bar)))
            .unwrap();
        TaskSystem::launch_task(TaskDefinition::new(count_task)).unwrap();
    }

    warn!("about to context switch");
    unsafe {
        asm!(
            "int {iv}",
            iv = const InterruptVector::ContextSwitch as u8,
        );
    }
}

fn count_task() {
    info!("in count task");
    for i in 0..100 {
        info!("{i}");
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
