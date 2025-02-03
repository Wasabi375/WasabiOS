extern crate proc_macro;

use args::TestArgs;
use proc_macro::TokenStream;
use syn::parse_macro_input;

mod args;
mod declaration;

/// Marks a function as a kernel test
///
/// This replaces rusts `test` proc macro.
///
/// # Arguments:
///
/// * name: <test name>
///     Rename the test
/// * expected_exit: <exit_value>
///     The test is expected to exit with the given [TestExitState]
/// * focus|x|f
///     If any test is marked as focused, then only focused tests are executed.
///     This can be used to override the ignore flag.
/// * ignore|i
///     The test is ignored
/// * multiprocessor|mp
///     The test is using multiple cores, an can take a [DataBarrier] input parameter
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
//// ```
#[proc_macro_attribute]
pub fn kernel_test(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attribute as TestArgs);

    let expanded = declaration::expand_test(args, parse_macro_input!(item));

    TokenStream::from(expanded)
}

/// Creates the distributed slice that is used to access all tests
/// marked by the [kernel_test] macro.
///
/// This must be invoked exactly once per crate in the top level module(lib.rs/main.rs)
#[proc_macro]
pub fn kernel_test_setup(item: TokenStream) -> TokenStream {
    assert!(item.is_empty());

    TokenStream::from(declaration::expand_slice())
}

/// Returns an iterator over all tests marked by the [kernel_test] macro.
///
/// # Arguments
///
/// The name of the crates containing tests as a comma separated list
#[proc_macro]
pub fn get_kernel_tests(item: TokenStream) -> TokenStream {
    let crates = parse_macro_input!(item);

    let expanded = declaration::expand_get_tests(crates);

    TokenStream::from(expanded)
}
