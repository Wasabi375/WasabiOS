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

use alloc::{boxed::Box, string::String, vec::Vec};
use bootloader_api::BootInfo;
use core::{
    any::Any,
    fmt::Debug,
    str::{from_utf8, FromStr},
    sync::atomic::{AtomicBool, Ordering},
};
use itertools::Itertools;
use shared::sync::{barrier::Barrier, lockcell::LockCell};
use testing::{
    description::{KernelTestDescription, KernelTestFunction, TestExitState, KERNEL_TESTS},
    multiprocessor::DataBarrier,
};
use uart_16550::SerialPort;
use wasabi_kernel::{
    bootloader_config_common,
    core_local::{get_ready_core_count, CoreInterruptState},
    mem::kernel_heap::KernelHeap,
    serial::SERIAL2,
    testing::{panic::use_custom_panic_handler, qemu},
    KernelConfig,
};

/// Write to the serial port and also trace the output
macro_rules! serial_line {
    ($serial:ident, $($arg:tt)+) => {{
        use core::fmt::Write;
        log::trace!(target: "test_serial::send", $($arg)+);
        core::writeln!($serial, $($arg)+)
    }};
}

/// configuration for the bootloader in test mode
const BOOTLOADER_CONFIG: bootloader_api::BootloaderConfig = {
    let config = bootloader_api::BootloaderConfig::new_default();
    bootloader_config_common(config)
};
const KERNEL_CONFIG: KernelConfig = KernelConfig { start_aps: true };
wasabi_kernel::entry_point!(
    kernel_test_main,
    boot_config = &BOOTLOADER_CONFIG,
    kernel_config = KERNEL_CONFIG
);

fn wait_for_test_ready_handshake(serial: &mut SerialPort) {
    debug!("waiting for test handshake");
    let _ = serial_line!(serial, "tests ready");

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

static TEST_START_BARRIER: DataBarrier<(KernelTestDescription, bool)> =
    DataBarrier::new(u32::max_value()).with_target_ordering(Ordering::Acquire);
static TEST_USER_DATA_BARRIER: DataBarrier<Box<dyn Any + Send>> =
    DataBarrier::new(u32::max_value()).with_target_ordering(Ordering::Acquire);
static TEST_MP_SUCCESS: AtomicBool = AtomicBool::new(true);
static TEST_END_BARRIER: Barrier =
    Barrier::new(u32::max_value()).with_target_ordering(Ordering::Acquire);
static TESTING_CORE_INTERRUPT_STATE: CoreInterruptState = CoreInterruptState;

fn init_mp() {
    testing::multiprocessor::init_interrupt_state(&TESTING_CORE_INTERRUPT_STATE);
    unsafe {
        // we call this on all processors with the same value so this will not affect
        // any processors currently waiting
        TEST_START_BARRIER.set_target(
            get_ready_core_count(Ordering::Acquire).into(),
            Ordering::Release,
        );
        TEST_USER_DATA_BARRIER.set_target(
            get_ready_core_count(Ordering::Acquire).into(),
            Ordering::Release,
        );
        TEST_END_BARRIER.set_target(
            get_ready_core_count(Ordering::Acquire).into(),
            Ordering::Release,
        );
    }
}

/// the main entry point for the kernel in test mode
fn kernel_test_main() -> ! {
    unsafe {
        locals!().disable_interrupts();
    }

    init_mp();

    if locals!().is_bsp() {
        bsp_main()
    } else {
        ap_main()
    }
}

fn ap_main() -> ! {
    loop {
        let test_guard = TEST_START_BARRIER
            .enter()
            .expect("failed to get test description");
        let (test, panicing) = test_guard.as_ref();
        let success = if *panicing {
            run_panicing_test(test)
        } else {
            run_test_no_panic(test)
        };

        if test.expected_exit == TestExitState::Succeed {
            // success if ALL processors succeed
            TEST_MP_SUCCESS.fetch_and(success, Ordering::SeqCst);
        } else {
            // success if ANY processor succeeds
            TEST_MP_SUCCESS.fetch_or(success, Ordering::SeqCst);
        }
        TEST_END_BARRIER.enter();
    }
}

fn bsp_main() -> ! {
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
        let line = read_line(serial);
        let line = line.as_slice();
        if log::log_enabled!(target: "test_serial::rec", log::Level::Trace) {
            if let Some(line_str) = from_utf8(line).ok() {
                trace!(target: "test_serial::rec", "{line_str}");
            } else {
                trace!(target: "test_serial::rec", "invalid utf8: {:?}", line);
            }
        }
        match line {
            b"count" => {
                let count = count_tests(false);
                let _ = serial_line!(serial, "{}", count);
            }
            b"count panic" => {
                let count = count_tests(true);
                let _ = serial_line!(serial, "{}", count);
            }
            b"count ignored" => {
                let count = count_ignored(false);
                let _ = serial_line!(serial, "{}", count);
            }
            b"count ignored panic" => {
                let count = count_ignored(true);
                let _ = serial_line!(serial, "{}", count);
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
            let _ = serial_line!(serial, "start test {test_number}");

            let success = run_test_bsp(test, panicing);
            if success {
                let _ = serial_line!(serial, "test succeeded");
            } else {
                let _ = serial_line!(serial, "test failed");
            }
            return success;
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
            let _ = serial_line!(serial, "start test {test_number}");
            debug!("send: start test {test_number}");
            let start_confirmation = read_line(serial);
            assert_eq!(start_confirmation.as_slice(), b"start test");

            if run_test_bsp(test, false) {
                success += 1;
                let _ = serial_line!(serial, "test succeeded");
                debug!("send test succeded");
            } else {
                let _ = serial_line!(serial, "test failed");
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
            if run_test_bsp(test, false) {
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

fn run_test_bsp(test: &KernelTestDescription, panicing: bool) -> bool {
    if test.multiprocessor {
        if test.expected_exit == TestExitState::Succeed {
            // we succeed if ALL processor succeeds, so default is true
            TEST_MP_SUCCESS.store(true, Ordering::Release);
        } else {
            // we succeed if ANY processor succeeds, so default is false
            TEST_MP_SUCCESS.store(false, Ordering::Release);
        }

        // we can reuse the original test and don't need to rely on the shared
        // data.
        let _shared_test = TEST_START_BARRIER
            .enter_with_data((test.clone(), panicing))
            .expect("sharing data should never fail");
    }

    #[cfg(feature = "mem-stats")]
    let mem_usage_befor = KernelHeap::get().lock().stats().snapshot();

    let mut success = if panicing {
        trace!("run panic-expected test");
        run_panicing_test(test);
        false
    } else {
        trace!("run normal test");
        run_test_no_panic(test)
    };

    if test.multiprocessor {
        TEST_END_BARRIER.enter();

        if test.expected_exit == TestExitState::Succeed {
            success = TEST_MP_SUCCESS.load(Ordering::SeqCst) & success;
        } else {
            // we only expect at least 1 processor to produce the correct failure,
            // meaning that exactly 1 processors test needs to succeed
            success = TEST_MP_SUCCESS.load(Ordering::SeqCst) | success;
        }

        TEST_MP_SUCCESS.store(true, Ordering::SeqCst);
    }

    TEST_USER_DATA_BARRIER.take_data();

    #[cfg(feature = "mem-stats")]
    {
        let mem_usage_after = KernelHeap::get().lock().stats().snapshot();
        if mem_usage_befor != mem_usage_after {
            let leaked = mem_usage_after.used as i128 - mem_usage_befor.used as i128;

            if test.allow_heap_leak {
                warn!("Memory leak detected! {leaked} bytes");
            } else {
                error!("Memory leak detected! {leaked} bytes");
                return false;
            }
        }
    }

    if panicing {
        assert!(!success);
        error!("Expected test to panic, but all processors completed");
    }
    success
}

fn run_test_no_panic(test: &KernelTestDescription) -> bool {
    assert!(test.expected_exit != TestExitState::Panic);
    info!(
        "TEST: {} \t\t {} @ {}",
        test.name, test.fn_name, test.test_location
    );

    let test_result = match test.test_fn {
        KernelTestFunction::Normal(test_fn) => test_fn(),
        KernelTestFunction::MPBarrier(test_fn) => test_fn(&TEST_USER_DATA_BARRIER),
    };

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

    let _panic_handler_guard = use_custom_panic_handler(|_info| {
        // Safety: we are in a panic handler so we are the only running code
        let serial2 = unsafe { &mut *SERIAL2.shatter() }.as_mut().unwrap();
        //info!("Test paniced as expected");
        use core::fmt::Write;
        let _ = writeln!(serial2, "test succeeded");

        qemu::exit(qemu::ExitCode::Success);
    });

    let test_result = match test.test_fn {
        KernelTestFunction::Normal(test_fn) => test_fn(),
        KernelTestFunction::MPBarrier(test_fn) => test_fn(&TEST_USER_DATA_BARRIER),
    };

    info!("finished panicing test with {:?}", test_result);

    false
}

#[cfg(feature = "test-tests")]
mod test_tests {
    use core::{any::Any, sync::atomic::Ordering};

    use alloc::boxed::Box;
    use log::debug;
    use testing::{
        description::TestExitState, kernel_test, multiprocessor::DataBarrier, t_assert,
        t_assert_eq, t_assert_ne, tfail, KernelTestError, TestUnwrapExt,
    };
    use wasabi_kernel::{core_local::get_ready_core_count, time::sleep_tsc};

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Fail)))]
    fn test_tfail() -> Result<(), KernelTestError> {
        tfail!()
    }

    #[kernel_test(allow_heap_leak)]
    fn test_kernel_heap_mem_leak() -> Result<(), KernelTestError> {
        let b = Box::new(5);
        core::mem::forget(b);
        Ok(())
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

    #[kernel_test(expected_exit: TestExitState::Error(Some(KernelTestError::Fail)))]
    fn test_t_unwrap_message() -> Result<(), KernelTestError> {
        let o: Option<()> = None;
        o.tunwrap()?;

        Ok(())
    }

    #[kernel_test(i)]
    fn ignored_test() -> Result<(), KernelTestError> {
        tfail!("ignored test should never execute");
    }

    // NOTE: this test needs to be commented out, otherwise it will be focused
    // diabling all other tests
    //    #[kernel_test(i, x)]
    //    fn focused_ignore_test() -> Result<(), KernelTestError> {
    //        Ok(())
    //    }

    #[kernel_test(mp)]
    fn test_multiprocessor_empty() -> Result<(), KernelTestError> {
        Ok(())
    }

    #[kernel_test(mp)]
    fn test_multiprocessor_barrier(
        db: &DataBarrier<Box<dyn Any + Send>>,
    ) -> Result<(), KernelTestError> {
        let data = if locals!().is_bsp() {
            let data = Box::new(2u64);
            db.enter_with_data(data)
        } else {
            db.enter()
        }
        .tunwrap()?;

        let value = data.downcast_ref::<u64>().tunwrap()?;
        t_assert_eq!(2, *value);

        Ok(())
    }

    #[kernel_test(mp, expected_exit: TestExitState::Panic)]
    fn test_multiprocessor_ap_panic() -> Result<(), KernelTestError> {
        if get_ready_core_count(Ordering::Acquire) == 1 {
            panic!("Test only works in mp environment");
        }
        sleep_tsc(shared::types::Duration::Seconds(1));

        debug!("in panicing test");
        if locals!().is_bsp() {
            Ok(())
        } else {
            panic!("expected panic on ap");
        }
    }

    #[kernel_test(mp, expected_exit: TestExitState::Error(None))]
    fn test_multiprocessor_ap_failure() -> Result<(), KernelTestError> {
        if get_ready_core_count(Ordering::Acquire) == 1 {
            tfail!("Test only works in mp environment");
        }

        if locals!().is_bsp() {
            Ok(())
        } else {
            tfail!("expected failure on ap");
        }
    }
}
