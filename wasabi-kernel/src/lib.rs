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
    cpu::{apic, cpuid, halt, interrupts},
};
use bootloader_api::{config::Mapping, BootInfo};
use core::ptr::null_mut;

/// The main function called for each Core after the kernel is initialized
static mut KERNEL_MAIN: fn() -> ! = halt;

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

/// enters the main kernel function, specified by the [entry_point] macro.
pub fn enter_kernel_main() -> ! {
    unsafe {
        // Safety: this is only written once during bsp start.
        // There is no way this is currently being modified
        KERNEL_MAIN()
    }
}

/// initializes the kernel and calls `kernel_start`.
pub fn kernel_bsp_entry(
    boot_info: &'static mut BootInfo,
    kernel_config: KernelConfig,
    kernel_start: fn() -> !,
) -> ! {
    // Safety: TODO: boot info handling not really save, but it's fine for now
    // we are still single core and don't really care
    unsafe {
        BOOT_INFO = boot_info;
    }
    unsafe {
        // Safety: only written once here, and bsp_entry is only executed once
        KERNEL_MAIN = kernel_start;
    }

    // Safety:
    // `init` is only called once per core and `core_boot`
    let core_id = unsafe { core_boot() };

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
        locals!().gdt.init_and_load();

        // Safety: bsp during `init` and locks, logging and alloc are working
        graphics::init(true);
    }

    interrupts::init();

    apic::init().unwrap();

    if kernel_config.start_aps {
        apic::ap_startup::ap_startup();
    }

    assert!(locals!().interrupts_enabled());
    info!("Kernel initialized");

    enter_kernel_main()
}

/// The default stack size used by the kernel
pub const DEFAULT_STACK_SIZE: u64 = KiB!(80);
// assert stack size is multiple of page size
const_assert!(DEFAULT_STACK_SIZE & 0xfff == 0);

/// The default number of pages(4K) used for the kernel stack
///
/// Calculated from [DEFAULT_STACK_SIZE]
pub const DEFAULT_STACK_PAGE_COUNT: u64 = DEFAULT_STACK_SIZE / Size4KiB::SIZE;

/// Configuration for the kernel start
pub struct KernelConfig {
    /// if set, application processors will be started as part of kernel start
    pub start_aps: bool,
}

/// fills in bootloader configuration that is shared between normal and test mode
pub const fn bootloader_config_common(
    mut config: bootloader_api::BootloaderConfig,
) -> bootloader_api::BootloaderConfig {
    config.mappings.page_table_recursive = Some(Mapping::Dynamic);
    config.kernel_stack_size = DEFAULT_STACK_SIZE;
    config
}

/// creates a default [KernelConfig].
pub const fn default_kernel_config() -> KernelConfig {
    KernelConfig { start_aps: true }
}

/// Creates the kernel entry point, which after initializing the kernel
/// will run the given function.
///
/// # Examples
/// ```
/// fn kernel_main() -> ! {
///     loop {}
/// }
/// const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
///     let config = bootloader_api::BootloaderConfig::new_default();
///     bootloader_config_common(config)
/// };
/// entry_point!(kernel_main);
/// entry_point!(kernel_main, boot_config = &BOOTLOADER_CONFIG);
/// entry_point!(kernel_main, kernel_config = default_kernel_config());
/// entry_point!(kernel_main, boot_config = ..., kernel_config = ...);
/// ```
#[macro_export]
macro_rules! entry_point {
    ($path:path) => {
        const _: () = {
            const __BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
                let config = bootloader_api::BootloaderConfig::new_default();
                bootloader_config_common(config)
            };
            $crate::entry_point!(
                $path,
                boot_config = __BOOTLOADER_CONFIG,
                kernel_config = $crate::default_kernel_config(),
            );
        };
    };
    ($path:path, boot_config = $config:expr $(,)?) => {
        $crate::entry_point!(
            $path,
            boot_config = $config,
            kernel_config = $crate::default_kernel_config(),
        );
    };
    ($path:path, kernel_config = $config:expr $(,)?) => {
        const _: () = {
            const __BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
                let config = bootloader_api::BootloaderConfig::new_default();
                bootloader_config_common(config)
            };
            $crate::entry_point!(
                $path,
                boot_config = __BOOTLOADER_CONFIG,
                kernel_config = $config,
            );
        };
    };
    ($path:path, kernel_config = $k_conf:expr, boot_config = $b_conf:expr $(,)?) => {
        $crate::entry_point!($path, boot_config = $b_conf, kernel_config = $k_conf);
    };
    ($path:path, boot_config = $b_conf:expr, kernel_config = $k_conf:expr $(,)?) => {
        #[doc(hidden)]
        fn __impl_kernel_start(boot_info: &'static mut BootInfo) -> ! {
            let kernel_conf: $crate::KernelConfig = $k_conf;
            let kernel_main: fn() -> ! = $path;
            $crate::kernel_bsp_entry(boot_info, kernel_conf, kernel_main);
        }
        bootloader_api::entry_point!(__impl_kernel_start, config = &BOOTLOADER_CONFIG);
    };
}
