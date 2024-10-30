//! Contains structs and traits used to describe a test.
//!
//! This module should mainly be used in the test runner or
//! in macros declaring tests.
use core::any::Any;

use alloc::boxed::Box;
use linkme::distributed_slice;

use crate::{multiprocessor::DataBarrier, KernelTestError};

// TODO use Termination trait as return type?
/// Function signature used for kernel test functions
pub type KernelTestFn = fn() -> Result<(), KernelTestError>;

/// Function signature used for kernel test functions that use multiprocessing
pub type KernelMPTestFn = fn(&DataBarrier<Box<dyn Any + Send>>) -> Result<(), KernelTestError>;

/// the different varaints of allowed kernel test functions
#[derive(Debug, Clone)]
pub enum KernelTestFunction {
    /// the standart variant
    Normal(KernelTestFn),
    /// variant used for mulitprocessor data syncronization
    MPBarrier(KernelMPTestFn),
}

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
    pub test_fn: KernelTestFunction,

    /// The source location of the test
    pub test_location: SourceLocation,

    /// If any test has this flag set, only tests with this flag will be executed
    pub focus: bool,

    /// Tests with this flag will only be executed, if they also have the
    /// [focus](KernelTestDescription::focus) flag
    pub ignore: bool,

    /// Tests with this flag will be run on all processor cores simultaneously.
    ///
    /// For the test to count as a success, all processors must return with `Ok`.
    /// However the [expected exit](KernelTestDescription::expected_exit) is somthing
    /// other than [TestExitState::Succeed] it is sufficent if one processor exits
    /// with the expected exit. In that case all other processors must exit with `Ok`.
    pub multiprocessor: bool,

    /// If set, this test will not fail if a kernel heap memory leak is detected
    pub allow_heap_leak: bool,
    /// If set, this test will not fail if a page leak is detected
    pub allow_page_leak: bool,
    /// If set, this test will not fail if a frame leak is detected
    pub allow_frame_leak: bool,
}

/// The distributed slice, collecting all kernel testss marked with `#[kernel_test]`
#[distributed_slice]
pub static KERNEL_TESTS: [KernelTestDescription] = [..];
