//! Library providing #[test] like functionaility for my kernel

#![no_std]
#![feature(error_in_core)]
// #![warn(missing_docs, rustdoc::missing_crate_level_docs)] // TODO enable
#![deny(unsafe_op_in_unsafe_fn)]

use linkme::distributed_slice;
use thiserror::Error;

/// Exit code for qemu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    /// kernel exit successfuly.
    Success = 0x10,
    /// an error occured
    Error = 0x11,
}

/// stop the kernel and exit qeum
pub fn exit_qemu(code: QemuExitCode) -> ! {
    use x86_64::instructions::port::Port;

    unsafe {
        let mut port = Port::new(0xf4);
        port.write(code as u32);
    }
    unreachable!();
}

/// Error type for Kernel-tests
#[derive(Error, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum KernelTestError {}

/// Function signature used for kernel test functions
pub type KernelTestFn = fn() -> Result<(), KernelTestError>;

/// Describes the possible ways a test can exit
#[derive(Debug, Clone)]
pub enum TestExitState {
    /// The test finishes normaly
    Succeed,
    /// The test panics
    Panic,
    // TODO page_fault, etc? Do I want this?
}

/// Description of a single kernel test
#[derive(Debug, Clone)]
pub struct KernelTestDescription {
    /// the module that contains the test
    pub module: &'static str,

    /// the name of the test
    pub name: &'static str,

    /// the name of the test function
    ///
    /// in most cases this is equal to [name](KernelTestDescription::name)
    pub fn_name: &'static str,

    /// the way we expect the test to exit
    pub expected_exit: TestExitState,
}

#[distributed_slice]
pub static KERNEL_TESTS: [KernelTestDescription] = [..];

#[distributed_slice(KERNEL_TESTS)]
static FOO: KernelTestDescription = KernelTestDescription {
    module: "foo",
    name: "foo",
    fn_name: "foo",
    expected_exit: TestExitState::Succeed,
};

#[inline(never)]
pub fn access_slice() {
    for _ in KERNEL_TESTS {
        todo!();
    }
}
