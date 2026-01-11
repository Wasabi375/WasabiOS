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
    maybe_uninit_array_assume_init,
    never_type,
    pointer_is_aligned_to,
    ptr_alignment_type,
    ptr_metadata,
    stmt_expr_attributes
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
pub mod task;
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
    KiB,
    cpu::time::timestamp_now_tsc,
    types::{CoreId, Duration},
};
use static_assertions::const_assert;
use time::time_since_tsc;
use x86_64::{
    PhysAddr, VirtAddr,
    structures::paging::{Page, PageSize, Size4KiB},
};

use crate::{
    core_local::core_boot,
    cpu::{acpi::ACPI, apic, cpuid, interrupts},
    mem::structs::Pages,
    task::TaskSystem,
};
use bootloader_api::{BootInfo, config::Mapping, info::Optional};
use core::{
    hint::spin_loop,
    sync::atomic::{AtomicU8, Ordering},
};

/// Function type for the main kernel function
pub type KernelMainFn = fn() -> ();

/// The main function called for each Core after the kernel is initialized
static mut KERNEL_MAIN: KernelMainFn = empty_main;
fn empty_main() {}

static KERNEL_MAIN_BARRIER: AtomicU8 = AtomicU8::new(0);

/// Returns true after the kernel is initialized and has entered it's main
/// function
pub fn in_kernel_main() -> bool {
    // TODO this can report false positives if this is called during processor_init before
    // core_local::init. I think I am fine with this, but maybe I am not?
    // Do I even need this if it is this flaky?
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
        KERNEL_MAIN();
    }

    unsafe {
        // Safety: kernel_main exited. Stack has a static lifetime
        TaskSystem::terminate_task();
    }
}

/// initializes the kernel and calls `kernel_start`.
pub fn kernel_bsp_entry(
    boot_info: &'static mut BootInfo,
    kernel_config: KernelConfig,
    kernel_start: KernelMainFn,
) -> ! {
    // NOTE it is not save to use KernelInfo::get before processor_init is finished
    // as it will take a mutable reference to the underlying data. Therefore we
    // copy the data we require before moving BootInfo into KernelInfo
    let kernel_stack_bottom = boot_info.kernel_stack_bottom;
    let kernel_stack_len = boot_info.kernel_stack_len;
    unsafe {
        // Saftey: This is the first function ever called
        KernelInfo::init(boot_info);

        // Safety: only written once here, and bsp_entry is only executed once
        KERNEL_MAIN = kernel_start;
    }

    time::calibrate_tsc();

    let stack = Pages::<Size4KiB> {
        first_page: Page::from_start_address(VirtAddr::new(kernel_stack_bottom))
            .expect("Bootloader should always start stack at a 4KiB page"),
        count: pages_required_for!(Size4KiB, kernel_stack_len),
    };
    unsafe {
        processor_init(stack);
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
pub unsafe fn processor_init(stack: Pages<Size4KiB>) {
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
            // Safety: called during bsp start after check_cpuid_usable and
            // we have no references to the result of get
            cpuid::CPUCapabilities::load();

            // Safety: bsp during `init` and locks and logging are working
            mem::init();
            // Safety: bsp during `init` right after mem is initialized
            crossbeam_epoch::bsp_init();
        }
        // Safety:
        // this is called after `core_boot()` and we have initialized memory and logging
        core_local::init(core_id, stack);

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

    unsafe {
        // Safety: once per core during init after apic::init
        locals!().task_system.init();
    }
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

/// The fixed address the kernel is loaded at
#[cfg(feature = "fixed-kernel-vaddr")]
pub const KERNEL_BINARY_VADDR: x86_64::VirtAddr = x86_64::VirtAddr::new(0xff0_0000_0000);

/// fills in bootloader configuration that is shared between normal and test mode
pub const fn bootloader_config_common(
    mut config: bootloader_api::BootloaderConfig,
) -> bootloader_api::BootloaderConfig {
    config.mappings.page_table_recursive = Some(Mapping::Dynamic);
    config.mappings.aslr = false;
    config.kernel_stack_size = DEFAULT_STACK_SIZE;

    #[cfg(feature = "fixed-kernel-vaddr")]
    {
        config.mappings.kernel_base = Mapping::FixedAddress(KERNEL_BINARY_VADDR.as_u64())
    }

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
            let kernel_main: $crate::KernelMainFn = $path;
            $crate::kernel_bsp_entry(boot_info, kernel_conf, kernel_main);
        }
        bootloader_api::entry_point!(__impl_kernel_start, config = $b_conf);
    };
}

#[cfg(feature = "test")]
testing::description::kernel_test_setup!();
