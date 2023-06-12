//! Kernel tests

#![no_std]
#![no_main]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

#[allow(unused_imports)]
#[macro_use]
extern crate wasabi_kernel;

use linkme::distributed_slice;
#[allow(unused_imports)]
use log::{debug, info, trace, warn};

use bootloader_api::BootInfo;
use testing::{KernelTestDescription, QemuExitCode, TestExitState};
use wasabi_kernel::{bootloader_config_common, init};

/// configuration for the bootloader in test mode
const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let config = bootloader_api::BootloaderConfig::new_default();
    bootloader_config_common(config)
};
bootloader_api::entry_point!(kernel_test, config = &BOOTLOADER_CONFIG);

/// the main entry point for the kernel in test mode
fn kernel_test(boot_info: &'static mut BootInfo) -> ! {
    init(boot_info);

    info!("Tests:");
    for test in testing::KERNEL_TESTS {
        info!("{test:#?}");
    }

    info!("Kernel tests done! cpu::halt()");

    testing::exit_qemu(QemuExitCode::Success);
}

#[distributed_slice(testing::KERNEL_TESTS)]
static BAR: KernelTestDescription = KernelTestDescription {
    module: "bar",
    name: "bar",
    fn_name: "bar",
    expected_exit: TestExitState::Succeed,
};
