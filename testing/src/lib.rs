//! Library providing #[test] like functionaility for my kernel

#![no_std]
#![feature(error_in_core)]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

pub use testing_derive::kernel_test;

use linkme::distributed_slice;
use thiserror::Error;

/// Error type for Kernel-tests
#[derive(Error, Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum KernelTestError {
    #[error("test failed")]
    Fail,
}

// TODO use Termination trait as return type?
/// Function signature used for kernel test functions
pub type KernelTestFn = fn() -> Result<(), KernelTestError>;

/// Describes the possible ways a test can exit
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestExitState {
    /// The test finishes normaly
    Succeed,
    /// The test finishes with the specified [KernelTestError] or any
    /// [KernelTestError] if `None`
    Error(Option<KernelTestError>),
    /// The test panics
    Panic,
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

    /// The source location of the test
    pub test_location: SourceLocation,

    /// If any test has this flag set, only tests with this flag will be executed
    pub focus: bool,

    /// Tests with this flag will only be executed, if they also have the
    /// [focus](KernelTestDescription::focus) flag
    pub ignore: bool,
}

/// The distributed slice, collecting all kernel testss marked with `#[kernel_test]`
#[distributed_slice]
pub static KERNEL_TESTS: [KernelTestDescription] = [..];
