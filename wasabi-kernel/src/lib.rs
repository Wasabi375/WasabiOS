//! a hobby os kernel written in Rust

#![no_std]
#![no_main]
#![feature(
    abi_x86_interrupt,
    allocator_api,
    ascii_char,
    assert_matches,
    box_into_inner,
    debug_closure_helpers,
    extern_types,
    let_chains,
    maybe_uninit_array_assume_init,
    maybe_uninit_uninit_array,
    never_type,
    pointer_is_aligned_to,
    ptr_alignment_type,
    result_flattening,
    stmt_expr_attributes,
    strict_overflow_ops,
    new_zeroed_alloc
)]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![allow(rustdoc::private_intra_doc_links)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod core_local;
pub mod cpu;
pub mod crossbeam_epoch;
pub mod graphics;
pub mod kernel_info;
pub mod logger;
pub mod math;
pub mod mem;
pub mod panic;
pub mod pci;
pub mod prelude;
pub mod serial;
pub mod time;
pub mod utils;

#[cfg(feature = "test")]
pub mod testing;

extern crate alloc;

use core_local::get_ready_core_count;
use kernel_info::KernelInfo;
#[allow(unused_imports)]
use log::{debug, info, trace, warn};
use shared::{
    cpu::time::timestamp_now_tsc,
    types::{CoreId, Duration},
    KiB,
};
use static_assertions::const_assert;
use time::time_since_tsc;
use x86_64::{
    structures::paging::{PageSize, Size4KiB},
    PhysAddr,
};

use crate::{
    core_local::core_boot,
    cpu::{acpi::ACPI, apic, cpuid, halt, interrupts},
};
use bootloader_api::{config::Mapping, info::Optional, BootInfo};
use core::{
    hint::spin_loop,
    sync::atomic::{AtomicU8, Ordering},
};

/// The main function called for each Core after the kernel is initialized
static mut KERNEL_MAIN: fn() -> ! = halt;

static KERNEL_MAIN_BARRIER: AtomicU8 = AtomicU8::new(0);

/// Returns true after the kernel is initialized and has entered it's main
/// function
pub fn in_kernel_main() -> bool {
    KERNEL_MAIN_BARRIER.load(Ordering::Acquire) == get_ready_core_count(Ordering::Acquire)
}

/// enters the main kernel function, specified by the [entry_point] macro.
///
/// # Safety
///
/// the processor must be in a valid state that does not randomly cause UB
pub unsafe fn enter_kernel_main() -> ! {
    KERNEL_MAIN_BARRIER.fetch_add(1, Ordering::SeqCst);
    let spin_start = timestamp_now_tsc();
    let mut warn_send = false;
    while KERNEL_MAIN_BARRIER.load(Ordering::SeqCst) != get_ready_core_count(Ordering::SeqCst) {
        spin_loop();
        if time_since_tsc(spin_start) > Duration::new_seconds(5) && !warn_send {
            warn!("waiting for all cores to reach kernel_main");
            warn_send = true;
        }
        if time_since_tsc(spin_start) > Duration::new_seconds(30) {
            panic!("cancel waiting fo rall cores to reach kernel_main");
        }
    }
    if locals!().is_bsp() {
        info!("all cores reached kernel_main!");
    }
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
    unsafe {
        // Saftey: This is the first function ever called
        KernelInfo::init(boot_info);

        // Safety: only written once here, and bsp_entry is only executed once
        KERNEL_MAIN = kernel_start;
    }

    time::calibrate_tsc();

    unsafe {
        processor_init();
    }

    if kernel_config.start_aps {
        apic::ap_startup::ap_startup();
    }

    assert!(locals!().interrupts_enabled());
    info!("Kernel initialized");

    unsafe {
        // Safety: core is initialized
        enter_kernel_main()
    }
}

/// Safety: should only be called once, right at the start
/// of the start of the processor
pub unsafe fn processor_init() {
    let core_id: CoreId;
    unsafe {
        // Safety: `init` is only called once per core and `core_boot`
        core_id = core_boot();

        if core_id.is_bsp() {
            // Safety: bsp during `init`
            serial::init_serial_ports();

            // Safety: bsp during `init` after serial::init_serial_ports
            logger::init();
        }

        // Safety: inherently unsafe and can crash, but if cpuid isn't supported
        // we will crash at some point in the future anyways, so we might as well
        // crash early
        cpuid::check_cpuid_usable();

        if core_id.is_bsp() {
            // Safety: bsp during `init` and locks and logging are working
            mem::init();
            // Safety: bsp during `init` right after mem is initialized
            crossbeam_epoch::bsp_init();
        }
        // Safety:
        // this is called after `core_boot()` and we have initialized memory and logging
        core_local::init(core_id);

        // Safety: called during `init` after `crossbeam_epoch::bsp_init` and `core_local::init`
        crossbeam_epoch::processor_init();

        // Safety:
        // only called here for bsp and we have just init core_locals and logging and mem
        locals!().gdt.init_and_load();

        // Saftey: during bsp init
        interrupts::init();
    }

    if core_id.is_bsp() {
        // TODO cleanup acpi init and store it

        if let Optional::Some(rsdp_addr) = KernelInfo::get().boot_info.rsdp_addr {
            let paddr = PhysAddr::new(rsdp_addr);
            ACPI::from_rsdp(paddr).expect("failed to init acpi");
        } else {
            panic!("Bootloader did not report rsdp address");
        }

        unsafe {
            // Safety: bsp during `init` and locks, logging and alloc are working
            graphics::init(true);
        }

        pci::init();
    }

    apic::init().unwrap();
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
    config.mappings.aslr = false;
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
