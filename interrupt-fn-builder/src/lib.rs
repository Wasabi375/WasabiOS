//! provides macros to create interrupt exception handler funcions.
//!
//! They take a fn name, parameter names and a function block as arguments.
//! The macros automatically generate the correct function signature and make
//! the necessary setup calls, to ensure that `CoreLocals` is in a valid interrupt
//! state.
//!
//! This libraray should only be used within wasabi-kernel or at least provide
//! aliases for the `locals!()` macro.
//!
//! The following builders allow for a custom handler implementation:
//! [interrupt_fn!], [exception_fn!], [interrupt_with_error_fn!],
//! [exception_with_error_fn!], [exception_page_fault_fn!],
//!
//! While the "panic" variants, simply panic and print the interrupt parameters
//! [panic_interrupt!], [panic_exception!], [panic_interrupt_with_error!],
//! [panic_exception_with_error!],

#![no_std]

use paste::paste;

/// a builder macro to create the interrupt fn builders
///
/// takes either `interrupt` or `exception` as an `$int_type` identifier.
#[doc(hidden)]
macro_rules! int_fn_builder {
    ($int_type:ident) => {
        paste! {
            /// creates a handler function for an $int_type
            #[macro_export]
            macro_rules! [<$int_type _fn>]  {
                ($pub:vis $name:ident, $ist_name:ident, $block:tt) => {
                    $pub extern "x86-interrupt" fn $name($ist_name: x86_64::structures::idt::InterruptStackFrame) {
                        let _guard = crate::locals!().[<inc_ $int_type>]();
                        $block
                    }
                };
            }

            /// creats a handler function for an $int_type which will panic
            #[macro_export]
            macro_rules! [<panic_ $int_type>]  {
                ($pub:vis $name:ident) => {
                    $pub extern "x86-interrupt" fn $name(isf: x86_64::structures::idt::InterruptStackFrame) {
                        let _guard = crate::locals!().[<inc_ $int_type>]();
                        panic!("{} {}\n{isf:#?}", stringify!([[<$int_type>]]), stringify!($name));
                    }
                };
            }


            /// creates a handler function for an $int_type
            #[macro_export]
            macro_rules! [<$int_type _with_error_fn>] {
                ($pub:vis $name:ident, $ist_name:ident, $err_name:ident, $block:tt) => {
                    $pub extern "x86-interrupt" fn $name($ist_name: x86_64::structures::idt::InterruptStackFrame, $err_name: u64) -> !  {
                        let _guard = crate::locals!().[<inc_ $int_type>]();
                        $block
                    }
                };
            }

            /// creats a handler function for an $int_type which will panic
            #[macro_export]
            macro_rules! [<panic_ $int_type _with_error>] {
                ($pub:vis $name:ident) => {
                    $pub extern "x86-interrupt" fn $name(st: x86_64::structures::idt::InterruptStackFrame, err: u64) {
                        let _guard = crate::locals!().[<inc_ $int_type>]();
                        panic!("{}\nerr: {err:?}\n{st:#?}", stringify!($name));
                    }
                };
            }
        }
    };
}

/// creates a page fault $int_type handler
#[macro_export]
macro_rules! exception_page_fault_fn {
    ($pub:vis $name:ident, $ist_name:ident, $err_name:ident, $block:tt) => {
        $pub extern "x86-interrupt" fn $name(
            $ist_name: x86_64::structures::idt::InterruptStackFrame,
            $err_name: x86_64::structures::idt::PageFaultErrorCode,
        ) {
            let _guard = crate::locals!().inc_exception();
            $block
        }
    };
}

#[macro_export]
macro_rules! diverging_exception_with_error_fn {
    ($pub:vis $name:ident, $ist_name:ident, $err_name:ident, $block:tt) => {
        $pub extern "x86-interrupt" fn $name(
            $ist_name: x86_64::structures::idt::InterruptStackFrame,
            $err_name: u64,
        ) -> ! {
            let _guard = crate::locals!().inc_exception();
            $block
        }
    };
}

#[macro_export]
macro_rules! panic_diverging_exception_with_error {
    ($pub:vis $name:ident) => {
        $pub extern "x86-interrupt" fn $name(
            st: x86_64::structures::idt::InterruptStackFrame,
            err: u64,
        ) -> ! {
            let _guard = crate::locals!().inc_exception();
            panic!("{}\nerr: {err:?}\n{st:#?}", stringify!($name));
        }
    };
}

#[macro_export]
macro_rules! diverging_exception {
    ($pub:vis $name:ident, $ist_name:ident, $block:tt) => {
        $pub extern "x86-interrupt" fn $name(
            $ist_name: x86_64::structures::idt::InterruptStackFrame,
        ) -> ! {
            let _guard = crate::locals!().inc_exception();
            $block
        }
    };
}

#[macro_export]
macro_rules! panic_diverging_exception {
    ($pub:vis $name:ident) => {
        $pub extern "x86-interrupt" fn $name(st: x86_64::structures::idt::InterruptStackFrame) -> ! {
            let _guard = crate::locals!().inc_exception();
            panic!("{}\n{st:#?}", stringify!($name));
        }
    };
}

int_fn_builder!(interrupt);
int_fn_builder!(exception);
