//! Kernel tests

#![no_std]
#![no_main]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

#[allow(unused_imports)]
#[macro_use]
extern crate wasabi_kernel;

use log::error;
#[allow(unused_imports)]
use log::{debug, info, trace, warn};

use bootloader_api::BootInfo;
use itertools::Itertools;
use linkme::distributed_slice;
use testing::{
    kernel_test, KernelTestDescription, KernelTestError, KernelTestFn, QemuExitCode, TestExitState,
    KERNEL_TESTS,
};
use wasabi_kernel::{bootloader_config_common, init};

/// configuration for the bootloader in test mode
const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let config = bootloader_api::BootloaderConfig::new_default();
    bootloader_config_common(config)
};
bootloader_api::entry_point!(kernel_test_main, config = &BOOTLOADER_CONFIG);

/// the main entry point for the kernel in test mode
fn kernel_test_main(boot_info: &'static mut BootInfo) -> ! {
    init(boot_info);

    let success = run_tests();

    info!("Kernel tests done! cpu::halt()");

    if success {
        testing::exit_qemu(QemuExitCode::Success);
    } else {
        testing::exit_qemu(QemuExitCode::Error);
    }
}

fn run_tests() -> bool {
    info!("Running kernel tests");
    let mut total_count = 0;
    let mut total_success = 0;
    for module in &KERNEL_TESTS
        .iter()
        .group_by(|desc| desc.test_location.module)
    {
        info!("Running tests in module {}", module.0);
        let mut count = 0;
        let mut success = 0;

        for test in module.1 {
            count += 1;
            if run_test(test) {
                success += 1;
            }
        }
        if count == success {
            info!("{}/{} tests in {} succeeded", success, count, module.0);
        } else {
            error!(
                "{}/{} tests in {} succeeded. {} tests failed.",
                success,
                count,
                module.0,
                count - success
            );
        }
        total_count += count;
        total_success += success;
    }
    if total_count == total_success {
        info!("{}/{} tests succeeded", total_success, total_count);
        true
    } else {
        error!(
            "{}/{} tests succeeded. {} tests failed.",
            total_success,
            total_count,
            total_count - total_success
        );
        false
    }
}

fn run_test(test: &KernelTestDescription) -> bool {
    info!(
        "TEST: {} \t\t {} @ {}",
        test.name, test.fn_name, test.test_location
    );

    let test_fn: KernelTestFn = test.test_fn;
    let test_result: Result<(), KernelTestError> = test_fn();
    match &test.expected_exit {
        TestExitState::Succeed => {
            if test_result.is_ok() {
                true
            } else {
                error!("TEST {}: failed with {:?}", test.name, test_result);
                false
            }
        }
        TestExitState::Error(error) => match test_result {
            Ok(()) => {
                error!(
                    "TEST {}: test-function succeeded but it was expected to fail with {:?}",
                    test.name, error
                );
                false
            }
            Err(test_result) => {
                if let Some(error) = error {
                    if *error == test_result {
                        true
                    } else {
                        error!(
                            "TEST {}: failed with {:?} but it was expected to fail with {:?}",
                            test.name, test_result, error
                        );
                        false
                    }
                } else {
                    true
                }
            }
        },
    }
}

#[kernel_test]
fn foobar() -> Result<(), KernelTestError> {
    Ok(())
}

#[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Fail)))]
fn failing() -> Result<(), KernelTestError> {
    Err(KernelTestError::Fail)
}
