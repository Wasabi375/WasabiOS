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
use wasabi_kernel::{
    KernelConfig, bootloader_config_common,
    cpu::{
        self,
        apic::{
            Apic,
            ipi::{self, Ipi},
        },
        interrupts::{InterruptVector, register_interrupt_handler},
    },
    default_kernel_config,
    prelude::LockCell,
    time::{self},
};

#[cfg(feature = "mem-stats")]
use wasabi_kernel::mem::{
    frame_allocator::FrameAllocator, kernel_heap::KernelHeap, page_allocator::PageAllocator,
    page_table::PageTable,
};
use x86_64::structures::idt::InterruptStackFrame;

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

    info!("OS Done!\tcpu::halt()");
    cpu::halt();
}

fn context_switch_experiment() {
    {
        register_interrupt_handler(InterruptVector::Test, context_switch_interrupt).unwrap();
    }

    assert!(!locals!().in_interrupt());

    let ipi = Ipi {
        mode: ipi::DeliveryMode::Fixed(InterruptVector::Test),
        destination: ipi::Destination::SelfOnly,
    };
    info!("about to send ipi");
    locals!().apic.lock().send_ipi(ipi);

    info!("interrupt returned");

    assert!(!locals!().in_interrupt());
    info!("everything good and normal");
}

fn context_switch_interrupt(
    vector: InterruptVector,
    stack_frame: InterruptStackFrame,
) -> Result<(), ()> {
    assert_eq!(vector, InterruptVector::Test);

    info!("test interrupt triggered!");

    info!("calling iretq");
    unsafe {
        // Safety:
        // manually handle eoi as we iretq and therefor interrupt_handler wrapper is not able
        // to do this
        Apic::eoi();

        // Safety: the decrement guard is never dropped, because of the iretq call. Manually
        // decrement instead
        locals!().interrupt_count.decrement();

        // Safety: valid stack frame
        stack_frame.iretq();
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
