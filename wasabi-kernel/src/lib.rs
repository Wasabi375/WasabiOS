//! a hobby os kernel written in Rust

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
    maybe_uninit_array_assume_init,
    stmt_expr_attributes,
    panic_info_message,
    box_into_inner,
    never_type
)]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![allow(rustdoc::private_intra_doc_links)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod core_local;
pub mod cpu;
pub mod graphics;
pub mod logger;
pub mod math;
pub mod mem;
pub mod panic;
pub mod prelude;
pub mod serial;
#[cfg(feature = "test")]
pub mod testing;
pub mod time;

extern crate alloc;

#[allow(unused_imports)]
use log::{debug, info, trace, warn};

use crate::{
    core_local::core_boot,
    cpu::{apic, cpuid, gdt, interrupts},
};
use bootloader_api::{config::Mapping, BootInfo};
use core::ptr::null_mut;

/// Contains the [BootInfo] provided by the Bootloader
/// TODO: this breaks rust uniquness guarantee
static mut BOOT_INFO: *mut BootInfo = null_mut();

/// returns the [BootInfo] provided by the bootloader
///
/// # Safety:
///
/// the caller must guarantee unique access
pub unsafe fn boot_info() -> &'static mut BootInfo {
    unsafe { &mut *BOOT_INFO }
}

/// initializes the kernel.
pub fn init(boot_info: &'static mut BootInfo) {
    // Safety: TODO: boot info handling not really save, but it's fine for now
    // we are still single core and don't really care
    unsafe {
        BOOT_INFO = boot_info;
    }

    // Safety:
    // `init` is only called once per core and `core_boot`
    let core_id = unsafe { core_boot() };

    if core_id.is_bsp() {
        time::calibrate_tsc();
        unsafe {
            // Safety: bsp during `init`
            serial::init_serial_ports();

            // Safety: bsp during `init` after serial::init_serial_ports
            logger::init();

            // Safety: inherently unsafe and can crash, but if cpuid isn't supported
            // we will crash at some point in the future anyways, so we might as well
            // crash early
            cpuid::check_cpuid_usable();

            // Safety: bsp during `init` and locks and logging are working
            mem::init();

            // Safety: bsp during `init` and locks, logging and alloc are working
            graphics::init(true);
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
    info!("Kernel initialized");
}

/// fills in bootloader configuration that is shared between normal and test mode
pub const fn bootloader_config_common(
    mut config: bootloader_api::BootloaderConfig,
) -> bootloader_api::BootloaderConfig {
    config.mappings.page_table_recursive = Some(Mapping::Dynamic);
    config
}
