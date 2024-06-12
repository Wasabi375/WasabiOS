//! Kernel tests
//!
//! The test kernel runs in 2 distinct modes. With or without serial control.
//! The test process can be controlled via simple commands sent on the COM2
//! serial port.  If there is a serial port the test kernel runs in serial
//! control mode. Otherwise it runs in no-serial mode.
//!
//! In no-serial mode the kernel will simply test all `#[kernel_test]`s that are not
//! marked as expected to panic. If a panic occures all following tests wont be
//! executed.
//!
//! However in some cases it is usefull to have tests that end in a panic (as a
//! success case) or to recover when a singel test unexpectedly panics.
//! In order to facilitate that the test kernel can run in serial-controll mode.
//!
//! After the kernel has started a simple handshake is performed.  The kernel
//! sends "tests ready\n" and waits until it receives "tests ready\n" back as a
//! response.
//! Once the handshake is done the kernel can be controlled with a few simple commands,
//! each ending in a new-line("\n").
//!
//! * "count":
//!     Counts all tests that are not expected to panic and returns the count on
//!     the serial port("{count}\n").
//! * "count panic":
//!     Counts all tests that are expected to panic and returns the count on the
//!     serial port("{count}\n").
//! * "count ignored"
//!     Counts all ignored tests that are not expected to panic and returns
//!     the count on the serial port("{count}\n")
//! * "count ignored panic"
//!     Counts all ignored tests that are expected to panic and returns
//!     the count on the serial port("{count}\n")
//!     
//! * "test normal"
//!     Runs all tests that are not expected to panic. At the start of each test
//!     "start test {test_number}" is send on the serial port. It then waits until
//!     it recieves "start test" to run the test. After the test is done
//!     either "test succeeded" or "test failed" is send on the serial port.
//!     `test_number` is a id (enumerated from 0) for each test that is stable for a
//!     given build but may change after the kernel is rebuild.
//!     test that are expected to panic and those that don't use independent enumerations,
//!     so there is a `test_number = 0` for both sets of tests.
//!
//!     Will exit qemu after all tests finished.
//!
//! * "resume"
//!     Expectes `start_at: usize` argument on separate line.
//!     
//!     Runs all tests that are not expected to panic starting at
//!     `test_number == start_at`.
//!     At the start of each test "start test {test_number}" is send on the
//!     serial port.  It then waits until it recieves "start test" to run the
//!     test. After the test is done either "test succeeded" or "test failed" is
//!     send on the serial port.
//!     `test_number` is a id (enumerated from 0) for each test that is stable
//!     for a given build but may change after the kernel is rebuild.  test that
//!     are expected to panic and those that don't use independent enumerations,
//!     so there is a `test_number = 0` for both sets of tests.
//!
//!     Will exit qemu after all tests finished.
//!
//! * "test panic"
//!     Expects `test_number: usize` argument on separate line.
//!
//!     Runs a single test that is expected to panic. Writes "start test {test_number}"
//!     to the serial port befor starting the test and will print either "test succeeded"
//!     or "test failed" after it depending on completion.
//!
//!     Will exit qemu after the test finished.
//!     
//! * "isolated"
//!     Expectes `test_number: usize` argument on separate line.
//!
//!     Runs a single test that is not expected to panic. Writes "start test {test_number}"
//!     to the serial port befor starting the test and will print either "test succeeded"
//!     or "test failed" after it depending on completion.
//!
//!     This can be used to isolate global state changes introduced by one test that might
//!     cause other tests to fail.
//!
//!     Will exit qemu after the test finished.

#![no_std]
#![no_main]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
#![feature(stmt_expr_attributes)]

#[allow(unused_imports)]
#[macro_use]
extern crate wasabi_kernel;

extern crate alloc;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use alloc::{string::String, vec::Vec};
use bootloader_api::BootInfo;
use core::{
    fmt::{Debug, Write},
    str::FromStr,
};
use itertools::Itertools;
use shared::lockcell::LockCell;
use testing::description::{KernelTestDescription, KernelTestFn, TestExitState, KERNEL_TESTS};
use uart_16550::SerialPort;
use wasabi_kernel::{
    bootloader_config_common,
    cpu::halt,
    serial::SERIAL2,
    testing::{panic::use_custom_panic_handler, qemu},
    KernelConfig,
};

/// configuration for the bootloader in test mode
const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let config = bootloader_api::BootloaderConfig::new_default();
    bootloader_config_common(config)
};
const KERNEL_CONFIG: KernelConfig = KernelConfig { start_aps: false };
wasabi_kernel::entry_point!(
    kernel_test_main,
    boot_config = &BOOTLOADER_CONFIG,
    kernel_config = KERNEL_CONFIG
);

fn wait_for_test_ready_handshake(serial: &mut SerialPort) {
    debug!("waiting for test handshake");
    let _ = writeln!(serial, "tests ready");

    const EXPECTED: &[u8] = b"tests ready\n";
    let mut response_position = 0;
    loop {
        let c = serial.receive();
        if c == EXPECTED[response_position] {
            response_position += 1;
        } else {
            response_position = 0;
        }
        if response_position == EXPECTED.len() {
            break;
        }
    }

    debug!("test handshake received");
}

/// the main entry point for the kernel in test mode
fn kernel_test_main() -> ! {
    unsafe {
        locals!().disable_interrupts();
    }
    if !locals!().is_bsp() {
        todo_warn!("Test do not support multiple cores");
        halt();
    }

    let mut serial = SERIAL2.lock();
    let success = if let Some(serial) = serial.as_mut() {
        run_tests(serial)
    } else {
        let success = run_tests_no_serial();
        info!("Kernel tests done! cpu::halt()");
        success
    };
    if success {
        qemu::exit(qemu::ExitCode::Success);
    } else {
        qemu::exit(qemu::ExitCode::Error);
    }
}

fn run_tests(serial: &mut SerialPort) -> bool {
    wait_for_test_ready_handshake(serial);

    loop {
        match read_line(serial).as_slice() {
            b"count" => {
                let count = count_tests(false);
                let _ = write!(serial, "{}\n", count);
            }
            b"count panic" => {
                let count = count_tests(true);
                let _ = write!(serial, "{}\n", count);
            }
            b"count ignored" => {
                let count = count_ignored(false);
                let _ = write!(serial, "{}\n", count);
            }
            b"count ignored panic" => {
                let count = count_ignored(true);
                let _ = write!(serial, "{}\n", count);
            }
            b"test normal" => {
                return run_tests_with_serial(serial, 0);
            }
            b"resume" => {
                let start_at = parse_line(serial);
                return run_tests_with_serial(serial, start_at);
            }
            b"test panic" => {
                trace!("panic test");
                let panic_test = parse_line(serial);
                return run_single_test(serial, panic_test, true);
            }
            b"isolated" => {
                trace!("isolated test");
                let panic_test = parse_line(serial);
                return run_single_test(serial, panic_test, false);
            }
            _ => panic!("unexpected command"),
        }
    }
}

fn run_single_test(serial: &mut SerialPort, test_to_run: usize, panicing: bool) -> bool {
    info!("Running isolated test");
    let focus_only = KERNEL_TESTS.iter().any(|t| t.focus);

    let mut test_number = 0;

    for module in &KERNEL_TESTS
        .iter()
        .group_by(|desc| desc.test_location.module)
    {
        for test in module.1 {
            if (test.expected_exit == TestExitState::Panic) != panicing {
                continue;
            }
            if focus_only && !test.focus {
                continue;
            }
            if test.ignore && !test.focus {
                continue;
            }

            if test_number < test_to_run {
                test_number += 1;
                continue;
            }

            info!("Running test in module {}", module.0);
            let _ = writeln!(serial, "start test {test_number}");

            return if panicing {
                trace!("run panic-expected test");
                run_panicing_test(test);
                let _ = writeln!(serial, "test failed");
                false
            } else {
                trace!("run normal test");
                if run_test_no_panic(test) {
                    let _ = writeln!(serial, "test succeeded");
                    true
                } else {
                    let _ = writeln!(serial, "test failed");
                    false
                }
            };
        }
    }

    panic!("test not found");
}

fn count_tests(expect_panic: bool) -> usize {
    let focus_only = KERNEL_TESTS.iter().any(|t| t.focus);
    KERNEL_TESTS
        .iter()
        // if there are focused tests, only count those
        .filter(|t| t.focus == focus_only)
        // skip tests that are ignored, unless they are also focused
        .filter(|t| !t.ignore || t.focus)
        // filter based on expected exit state
        .filter(|t| (t.expected_exit == TestExitState::Panic) == expect_panic)
        .count()
}

fn count_ignored(expect_panic: bool) -> usize {
    KERNEL_TESTS
        .iter()
        // count tests that are ignored (focused tests are not counted)
        .filter(|t| t.ignore && !t.focus)
        // filter based on expected exit state
        .filter(|t| (t.expected_exit == TestExitState::Panic) == expect_panic)
        .count()
}

fn read_line(serial: &mut SerialPort) -> Vec<u8> {
    let mut line = Vec::<u8>::new();
    loop {
        let b = serial.receive();
        if b == b'\n' {
            return line;
        }
        line.push(b);
    }
}

fn parse_line<P: FromStr>(serial: &mut SerialPort) -> P
where
    P::Err: Debug,
{
    let line = String::from_utf8(read_line(serial)).expect("expected valid utf8");
    line.parse().expect("failed to parse data")
}

fn run_tests_with_serial(serial: &mut SerialPort, start_at: usize) -> bool {
    info!("Running kernel tests");
    if start_at > 0 {
        debug!("starting at test {start_at}");
    }
    let mut total_count = 0;
    let mut total_success = 0;

    let focus_only = KERNEL_TESTS.iter().any(|t| t.focus);

    let mut test_number = 0;

    for module in &KERNEL_TESTS
        .iter()
        .group_by(|desc| desc.test_location.module)
    {
        let mut count = 0;
        let mut success = 0;

        info!("Running tests in module {}", module.0);
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

            // skip tests until we reach start_at
            if test_number < start_at {
                test_number += 1;
                continue;
            }

            count += 1;
            let _ = writeln!(serial, "start test {test_number}");
            debug!("send: start test {test_number}");
            let start_confirmation = read_line(serial);
            assert_eq!(start_confirmation.as_slice(), b"start test");

            if run_test_no_panic(test) {
                success += 1;
                let _ = writeln!(serial, "test succeeded");
                debug!("send test succeded");
            } else {
                let _ = writeln!(serial, "test failed");
                debug!("send test failed");
            }

            test_number += 1;
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

fn run_tests_no_serial() -> bool {
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
                error!("{}", error);
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

fn run_panicing_test(test: &KernelTestDescription) -> bool {
    assert!(test.expected_exit == TestExitState::Panic);
    info!(
        "TEST: {} \t\t {} @ {}",
        test.name, test.fn_name, test.test_location
    );

    let test_fn: KernelTestFn = test.test_fn;

    let _panic_handler_guard = use_custom_panic_handler(|_info| {
        use shared::lockcell::LockCellInternal;
        // Safety: we are in a panic handler so we are the only running code
        let serial2 = unsafe { SERIAL2.get_mut() }.as_mut().unwrap();
        info!("Test paniced as expected");
        let _ = writeln!(serial2, "test succeeded");
        qemu::exit(qemu::ExitCode::Success);
    });

    let test_result = test_fn();
    error!(
        "Expected test to panic, but it exited with {:?}",
        test_result
    );
    false
}

#[cfg(feature = "test-tests")]
mod test_tests {
    use testing::{
        description::TestExitState, kernel_test, t_assert, t_assert_eq, t_assert_ne, tfail,
        KernelTestError,
    };

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Fail)))]
    fn test_tfail() -> Result<(), KernelTestError> {
        tfail!()
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Fail)))]
    fn test_tfail_message() -> Result<(), KernelTestError> {
        tfail!("with message: {}", "fail variable");
    }

    #[kernel_test]
    fn test_t_assert_true() -> Result<(), KernelTestError> {
        t_assert!(true);
        t_assert!(true, "This is a message that we should never see!");
        t_assert!(
            true,
            "This is a message that we should never see with args: {}!",
            12 * 5
        );

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_false() -> Result<(), KernelTestError> {
        let foo = false;
        t_assert!(foo);
        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_false_message() -> Result<(), KernelTestError> {
        t_assert!(false, "Message: {}", 5 * 12);
        Ok(())
    }

    #[kernel_test]
    fn test_t_assert_eq() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_eq!(a, 2 * 21);
        t_assert_eq!(a, 2 * 21, "some message");
        t_assert_eq!(a, 2 * 21, "some message with args: {}, {}", a, 42);

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_eq_fail() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_eq!(a, 2 * 22);

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_eq_fail_message() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_eq!(a, 2 * 22, "Some message {a}");

        Ok(())
    }

    #[kernel_test]
    fn test_t_assert_ne() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_ne!(a, 2 * 22);
        t_assert_ne!(a, 2 * 22, "some message");
        t_assert_ne!(a, 2 * 22, "some message with args: {}, {}", a, 42);

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_ne_fail() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_ne!(a, 2 * 21);

        Ok(())
    }

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Assert)))]
    fn test_t_assert_ne_fail_message() -> Result<(), KernelTestError> {
        let a = 42;
        t_assert_ne!(a, 2 * 21, "Some message {a}");

        Ok(())
    }
}
