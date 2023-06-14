//! Library providing #[test] like functionaility for my kernel

#![no_std]
#![feature(error_in_core)]
// #![warn(missing_docs, rustdoc::missing_crate_level_docs)] // TODO enable
#![deny(unsafe_op_in_unsafe_fn)]

use core::fmt::Pointer;

pub use testing_derive::kernel_test;

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
#[derive(Error, Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum KernelTestError {
    #[error("test failed")]
    Fail,
}

/// Function signature used for kernel test functions
pub type KernelTestFn = fn() -> Result<(), KernelTestError>;

/// Describes the possible ways a test can exit
#[derive(Debug, Clone)]
pub enum TestExitState {
    /// The test finishes normaly
    Succeed,
    /// The test finishes with the specified [KernelTestError] or any
    /// [KernelTestError] if `None`
    Error(Option<KernelTestError>),
    // /// The test panics
    // TODO Panic,
    // TODO page_fault, etc? Do I want this?
}

/// The source code location of a test function
#[derive(Debug, Clone)]
pub struct SourceLocation {
    /// module of the source location
    pub module: &'static str,
    /// file of the source location
    pub file: &'static str,
    /// line of the source location
    pub line: u32,
    /// column of the source location
    pub column: u32,
}

impl core::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("{}:{}:{}", self.file, self.line, self.column))
    }
}

/// Description of a single kernel test
#[derive(Debug, Clone)]
pub struct KernelTestDescription {
    /// the name of the test
    pub name: &'static str,

    /// the name of the test function
    ///
    /// in most cases this is equal to [name](KernelTestDescription::name)
    pub fn_name: &'static str,

    /// the way we expect the test to exit
    pub expected_exit: TestExitState,

    /// the actual test
    pub test_fn: KernelTestFn,

    pub test_location: SourceLocation,
}

#[distributed_slice]
pub static KERNEL_TESTS: [KernelTestDescription] = [..];
