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

use bootloader_api::BootInfo;
use shared::sync::lockcell::LockCell;
use wasabi_kernel::{
    bootloader_config_common,
    cpu::{self},
    default_kernel_config, time, KernelConfig,
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
        // pci::pci_experiment();
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

    #[cfg(feature = "freeze-heap")]
    if locals!().is_bsp() {
        use alloc::boxed::Box;
        use wasabi_kernel::mem::kernel_heap::{freeze_global_heap, try_unfreeze_global_heap};

        freeze_global_heap().unwrap();

        let foo = core::hint::black_box(Box::new(5));
        drop(foo);

        info!("unfreeze");
        try_unfreeze_global_heap().unwrap();
    }

    info!("OS Done!\tcpu::halt()");
    cpu::halt();
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
