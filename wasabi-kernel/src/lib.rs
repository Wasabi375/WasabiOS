//! a hobby os kernel written in Rust

#![no_std]
#![no_main]
#![feature(
    abi_x86_interrupt,
    allocator_api,
    ascii_char,
    assert_matches,
    box_into_inner,
    error_in_core,
    let_chains,
    maybe_uninit_array_assume_init,
    maybe_uninit_uninit_array,
    never_type,
    panic_info_message,
    pointer_is_aligned,
    ptr_alignment_type,
    result_flattening,
    stmt_expr_attributes
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
use shared::KiB;
use static_assertions::const_assert;
use x86_64::structures::paging::{PageSize, Size4KiB};

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
pub fn kernel_init(boot_info: &'static mut BootInfo) {
    // Safety: TODO: boot info handling not really save, but it's fine for now
    // we are still single core and don't really care
    unsafe {
        BOOT_INFO = boot_info;
    }

    // Safety:
    // `init` is only called once per core and `core_boot`
    let core_id = unsafe { core_boot() };

    if !core_id.is_bsp() {
        panic!("not bsp");
    }

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

        // Safety:
        // this is called after `core_boot()` and we have initialized memory and logging
        core_local::init(core_id);

        // Safety:
        // only called here for bsp and we have just init core_locals and logging and mem
        locals!().gdt.init();

        // Safety: bsp during `init` and locks, logging and alloc are working
        graphics::init(true);
    }

    interrupts::init();

    apic::init().unwrap();

    apic::multiprocessor::ap_startup();

    assert!(locals!().interrupts_enabled());
    info!("Kernel initialized");
}

/// The default stack size used by the kernel
pub const DEFAULT_STACK_SIZE: u64 = KiB!(80);
// assert stack size is multiple of page size
const_assert!(DEFAULT_STACK_SIZE & 0xfff == 0);

/// The default number of pages(4K) used for the kernel stack
///
/// Calculated from [DEFAULT_STACK_SIZE]
pub const DEFAULT_STACK_PAGE_COUNT: u64 = DEFAULT_STACK_SIZE / Size4KiB::SIZE;

/// fills in bootloader configuration that is shared between normal and test mode
pub const fn bootloader_config_common(
    mut config: bootloader_api::BootloaderConfig,
) -> bootloader_api::BootloaderConfig {
    config.mappings.page_table_recursive = Some(Mapping::Dynamic);
    config.kernel_stack_size = DEFAULT_STACK_SIZE;
    config
}
