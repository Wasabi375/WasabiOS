//! Kernel tests
#![no_std]
#![no_main]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![feature(stmt_expr_attributes)]

#[allow(unused_imports)]
#[macro_use]
extern crate wasabi_kernel;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use bootloader_api::BootInfo;
use core::fmt::Write;
use itertools::Itertools;
use shared::lockcell::LockCell;
use testing::{
    kernel_test, KernelTestDescription, KernelTestError, KernelTestFn, TestExitState, KERNEL_TESTS,
};
use uart_16550::SerialPort;
use wasabi_kernel::{bootloader_config_common, init, serial::SERIAL2, testing::qemu};

/// configuration for the bootloader in test mode
const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let config = bootloader_api::BootloaderConfig::new_default();
    bootloader_config_common(config)
};
bootloader_api::entry_point!(kernel_test_main, config = &BOOTLOADER_CONFIG);

fn wait_for_test_ready_handshake(port: &mut SerialPort) {
    let _ = write!(port, "tests ready");

    const EXPECTED: &[u8] = b"start testing";
    let mut response_position = 0;
    loop {
        let c = port.receive();
        if c == EXPECTED[response_position] {
            response_position += 1;
        } else {
            response_position = 0;
        }
        if response_position == EXPECTED.len() {
            break;
        }
    }
}

/// the main entry point for the kernel in test mode
fn kernel_test_main(boot_info: &'static mut BootInfo) -> ! {
    init(boot_info);

    unsafe {
        locals!().disable_interrupts();
    }

    // TODO make SERIAL port optional
    // TODO only run no_panic tests if port doesn't exist
    // that way we don't have to abstract over windows/linux and can use linux only
    // and have a good set of tests if serial fails
    // let mut test_serial = SERIAL2.lock();

    // wait_for_test_ready_handshake(&mut test_serial);

    let success = run_tests_no_panic();

    info!("Kernel tests done! cpu::halt()");

    if success {
        // let _ = write!(test_serial, "success");
        qemu::exit(qemu::ExitCode::Success);
    } else {
        qemu::exit(qemu::ExitCode::Error);
    }
}

fn run_tests_no_panic() -> bool {
    info!("Running kernel tests");
    let mut total_count = 0;
    let mut total_success = 0;

    let focus_only = KERNEL_TESTS.iter().any(|t| t.focus);

    for module in &KERNEL_TESTS
        .iter()
        .group_by(|desc| desc.test_location.module)
    {
        info!("Running tests in module {}", module.0);
        let mut count = 0;
        let mut success = 0;

        for test in module.1 {
            if test.expected_exit == TestExitState::Panic {
                continue;
            }
            if focus_only && !test.focus {
                continue;
            }
            if test.ignore && !test.focus {
                continue;
            }
            count += 1;
            if run_test_no_panic(test) {
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

fn run_test_no_panic(test: &KernelTestDescription) -> bool {
    assert!(test.expected_exit != TestExitState::Panic);
    info!(
        "TEST: {} \t\t {} @ {}",
        test.name, test.fn_name, test.test_location
    );

    let test_fn: KernelTestFn = test.test_fn;
    let test_result = test_fn();

    return match test.expected_exit {
        TestExitState::Succeed => match test_result {
            Ok(()) => true,
            Err(error) => {
                error!("TEST failed with {}", error);
                false
            }
        },
        TestExitState::Error(None) => match test_result {
            Ok(()) => {
                error!("TEST terminated sucessfuly, but an error was expected");
                false
            }
            Err(_error) => true,
        },
        TestExitState::Error(Some(ref expected)) => match test_result {
            Ok(()) => {
                error!("TEST terminated sucessfuly, but an error was expected");
                false
            }
            Err(error) => {
                if error == *expected {
                    true
                } else {
                    error!(
                        "TEST terminated with Err({}) but Err({}) was expected",
                        error, expected
                    );
                    false
                }
            }
        },

        TestExitState::Panic => unreachable!(),
    };
}

#[kernel_test]
fn foobar() -> Result<(), KernelTestError> {
    Ok(())
}

#[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Fail)))]
fn failing() -> Result<(), KernelTestError> {
    Err(KernelTestError::Fail)
}

#[kernel_test( expected_exit: TestExitState::Panic)]
fn panicing() -> Result<(), KernelTestError> {
    panic!("Panic in test");
}
