#![feature(proc_macro_diagnostic)]

extern crate proc_macro;

#[allow(unused_imports)] // this disables the warning about the "test" in the module name
mod kernel_test;

mod multitest;

use proc_macro::TokenStream;
use syn::parse_macro_input;

/// Marks a function as a kernel test
///
/// This replaces rusts `test` proc macro.
///
/// # Arguments:
///
/// * name: <test name>
///   Rename the test
/// * expected_exit: <exit_value>
///   The test is expected to exit with the given [TestExitState]
/// * focus|x|f
///   If any test is marked as focused, then only focused tests are executed.
///   This can be used to override the ignore flag.
/// * ignore|i
///   The test is ignored
/// * multiprocessor|mp
///   The test is using multiple cores, an can take a [DataBarrier] input parameter
/// * allow_{}_leak frame/page/heap/mapping
///   The test is considered successfull even if there is a memory leak of the given type
///
///
/// # Setup
///
/// In order to call this macro the [kernel_test_setup] macro must be invoked once
/// for each crate using this macro.
///
/// # Example
/// ```
/// #[kernel_test]
/// fn test_fn() -> Result<(), KernelTestError> {}
///
/// #[kernel_test(name: test_2)]
/// fn test_with_custom_name() -> Result<(), KernelTestError> {}
///
/// #[kernel_test(i f)]
/// fn ignored_and_focused_test() -> Result<(), KernelTestError> {}
///
/// #[kernel_test(mp)]
/// fn mp_test() -> Result<(), KernelTestError> {}
///
/// #[kernel_test(mp)]
/// fn mp_test_with_barrier(db: &DataBarrier<Box<dyn Any + Send>>) -> Result<(), KernelTestError> {}
/// ```
#[proc_macro_attribute]
pub fn kernel_test(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attribute as kernel_test::args::TestArgs);

    let expanded = match kernel_test::declaration::expand_test(args, parse_macro_input!(item)) {
        Ok(expanded) => expanded,
        Err(err) => err.to_compile_error(),
    };

    TokenStream::from(expanded)
}

/// Creates the distributed slice that is used to access all tests
/// marked by the [kernel_test] macro.
///
/// This must be invoked exactly once per crate in the top level module(lib.rs/main.rs)
#[proc_macro]
pub fn kernel_test_setup(item: TokenStream) -> TokenStream {
    assert!(item.is_empty());

    TokenStream::from(kernel_test::declaration::expand_slice())
}

/// Returns an iterator over all tests marked by the [kernel_test] macro.
///
/// # Arguments
///
/// The name of the crates containing tests as a comma separated list
#[proc_macro]
pub fn get_kernel_tests(item: TokenStream) -> TokenStream {
    let crates = parse_macro_input!(item);

    let expanded = kernel_test::declaration::expand_get_tests(crates);

    TokenStream::from(expanded)
}

/// Marks a module as a multi-test module
///
/// This will generate "rust-tests" for each [kernel_test] directly defined in the module.
#[proc_macro_attribute]
pub fn multitest(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attribute as multitest::args::MultiTestArgs);

    let expanded = match multitest::declaration::expand_multitest(args, parse_macro_input!(item)) {
        Ok(expanded) => expanded,
        Err(err) => err.to_compile_error(),
    };

    TokenStream::from(expanded)
}
