//! Implementation shared by all kernel packages
#![no_std]
#![feature(
    box_into_inner,
    const_ops,
    const_trait_impl,
    downcast_unchecked,
    let_chains,
    maybe_uninit_slice,
    maybe_uninit_uninit_array,
    negative_impls,
    step_trait
)]
#![warn(missing_docs)]

extern crate alloc;

pub mod alloc_ext;
pub mod cpu;
pub mod math;
pub mod primitive_enum;
pub mod rangeset;
pub mod sync;
pub mod task_local;
pub mod types;

/// Calculat the number of bytes in n kilo bytes
#[macro_export]
macro_rules! KiB {
    ($v:expr) => {
        $v * 1024
    };
}

/// Calculat the number of bytes in n mega bytes
#[macro_export]
macro_rules! MiB {
    ($v:expr) => {
        $v * 1024 * 1024
    };
}

/// Calculat the number of bytes in n giga bytes
#[macro_export]
macro_rules! GiB {
    ($v:expr) => {
        $v * 1024 * 1024 * 1024
    };
}

/// Calculates the total number of *entries* to cover a *total size*
///
/// # Example
/// ```no_run
/// # #[macro_use] extern crate shared;
/// # fn main() {
/// # use static_assertions::const_assert_eq;
/// const_assert_eq!(1, counts_required_for!(512, 512));
/// const_assert_eq!(1, counts_required_for!(512, 8));
/// const_assert_eq!(2, counts_required_for!(512, 1024));
/// const_assert_eq!(1, counts_required_for!(512, size_of::<u32>() * 5));
/// # }
/// ```
#[macro_export]
macro_rules! counts_required_for {
    ($entry_size:expr, $total_size:expr) => {
        ($total_size / $entry_size)
            + if ($total_size % $entry_size != 0) {
                1
            } else {
                0
            }
    };
}

/// A macro logging and returning the result of any expression.
/// The result of the expression is logged using the [log::debug] macro.
///
/// ```
/// assert_eq!(5 + 3, dbg!(8)); // also calls log::debug("5 + 3 = 8")
/// ```
#[allow(unused_macros)]
#[macro_export]
macro_rules! dbg {
    ($v:expr) => {{
        let value = $v;
        log::debug!("{} = {:?}", core::stringify!($v), value);
        value
    }};
}

/// Same as [todo!] but only calls a [log::warn] instead of [panic].
#[allow(unused_macros)]
#[macro_export]
macro_rules! todo_warn {
    () => {
        log::warn!("not yet implemented")
    };
    ($($arg:tt)+) => {
        log::warn!(
            "not yet implemented: {}",
            $crate::__macro_internals::format_args!($($arg)+)
        )
    };
}

/// Same as [todo!] but only calls a [log::error] instead of [panic].
#[allow(unused_macros)]
#[macro_export]
macro_rules! todo_error {
    () => {
        log::error!("not yet implemented")
    };
    ($($arg:tt)+) => {
        log::error!(
            "not yet implemented: {}",
            $crate::__macro_internals::format_args!($($arg)+)
        )
    };
}

#[doc(hidden)]
pub mod __macro_internals {
    pub use core::format_args;
}
