//! Library providing #[test] like functionaility for my kernel

#![no_std]
#![feature(error_in_core)]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use core::{error::Error, fmt::Debug};

pub use testing_derive::kernel_test;
use thiserror::Error;

pub mod description;

/// Error type for Kernel-tests
#[derive(Error, Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum KernelTestError {
    #[error("TEST failed")]
    Fail,
    #[error("TEST assertion failed")]
    Assert,
}

/// Extensio for the Result/Option type useful for tests
pub trait TestUnwrapExt<T> {
    /// Same as `unwrap` but returns [KernelTestError]
    ///
    /// This should be called with the question mark operator:
    /// ```result.tunwrap()?```
    fn tunwrap(self) -> Result<T, KernelTestError>;
    /// Same as `expect` but returns [KernelTestError]
    ///
    /// This should be called with the question mark operator:
    /// ```result.texpect("error")?```
    fn texpect(self, message: &str) -> Result<T, KernelTestError>;
}

// TODO replace tfail! with custom error message
//  and use core::intrinsics::caller_location.
//  Otherwise we report the wrong code location.
//  Does that work with panic=abort
//  This probably also needs #[track_caller] and I might want
//  to use #[inline]
impl<T, E: Error> TestUnwrapExt<T> for Result<T, E> {
    fn tunwrap(self) -> Result<T, KernelTestError> {
        match self {
            Ok(v) => Ok(v),
            Err(err) => tfail!("{}", err),
        }
    }

    fn texpect(self, message: &str) -> Result<T, KernelTestError> {
        match self {
            Ok(v) => Ok(v),
            Err(_err) => tfail!("{}", message),
        }
    }
}

impl<T> TestUnwrapExt<T> for Option<T> {
    fn tunwrap(self) -> Result<T, KernelTestError> {
        match self {
            Some(v) => Ok(v),
            None => tfail!("Expected Some found None"),
        }
    }

    fn texpect(self, message: &str) -> Result<T, KernelTestError> {
        match self {
            Some(v) => Ok(v),
            None => tfail!("{}", message),
        }
    }
}

/// A wrapper that implements [core::fmt::Display] via the underlying types [core::fmt::Debug]
pub struct DisplayDebug<T: Debug>(pub T);

impl<T: Debug> core::fmt::Debug for DisplayDebug<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: Debug> core::fmt::Display for DisplayDebug<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        <T as Debug>::fmt(&self.0, f)
    }
}

impl<T: Debug> core::error::Error for DisplayDebug<T> {}

/// A extension trait for `Result<T, Debug>` which mapps the debug error value
/// in [DisplayDebug].
pub trait DebugErrResultExt<T, E: Debug> {
    /// wraps the Err variant in [DisplayDebug]
    fn map_err_debug_display(self) -> Result<T, DisplayDebug<E>>;
}

impl<T, E: Debug> DebugErrResultExt<T, E> for Result<T, E> {
    fn map_err_debug_display(self) -> Result<T, DisplayDebug<E>> {
        self.map_err(|e| DisplayDebug(e))
    }
}

/// Fails the current test.
/// Only useable if function returns `Result<T, KernelTestError`
#[macro_export]
macro_rules! tfail {
    () => {{
        log::error!(
            target: "TEST",
            "at {}\nFail",
            $crate::location_str!()
        );
        return Err($crate::KernelTestError::Fail)
    }};
    ($($arg:tt)+) => {{
        log::error!(
            target: "TEST",
            "at {}\nFailure: {}",
            $crate::location_str!(),
            $crate::__macro_internals::format_args!($($arg)+)
        );
        return Err($crate::KernelTestError::Fail)
    }};
}

/// Same as `assert!` but returns a [KernelTestError] instead of `panic`
#[macro_export]
macro_rules! t_assert {
    ($assertion:expr) => {
        if !$assertion {
            log::error!(
                target: "TEST",
                "at {}\nAssertion failed\n{}",
                $crate::location_str!(),
                $crate::__macro_internals::stringify!($assertion),
            );
            return Err($crate::KernelTestError::Assert);
        }
    };
    ($assertion:expr, $($arg:tt)+) => {
        if !$assertion {
            log::error!(
                target: "TEST",
                "at {}\nAssertion failed: {}\n{}",
                $crate::location_str!(),
                $crate::__macro_internals::format_args!($($arg)+),
                $crate::__macro_internals::stringify!($assertion),
            );
            return Err($crate::KernelTestError::Assert);
        }
    };
}

/// Same as `assert_eq!` but returns a [KernelTestError] instead of `panic`
#[macro_export]
macro_rules! t_assert_eq {
    ($left:expr, $right:expr) => {
        match (&$left, &$right) {
            (l_val, r_val) => {
                if !(l_val == r_val) {
                    log::error!(
                        target: "TEST",
                        "at {}\nAssertion left == right failed\nleft: {} = {:?}\nright: {} = {:?}",
                        $crate::location_str!(),
                        $crate::__macro_internals::stringify!($left),
                        l_val,
                        $crate::__macro_internals::stringify!($right),
                        r_val
                    );
                    return Err($crate::KernelTestError::Assert);
                }
            }
        }
    };
    ($left:expr, $right:expr, $($arg:tt)+) => {
        match (&$left, &$right) {
            (l_val, r_val) => {
                if !(l_val == r_val) {
                    log::error!(
                        target: "TEST",
                        "at {}\nAssertion left == right failed: {}\nleft: {} = {:?}\nright: {} = {:?}",
                        $crate::location_str!(),
                        $crate::__macro_internals::format_args!($($arg)+),
                        $crate::__macro_internals::stringify!($left),
                        l_val,
                        $crate::__macro_internals::stringify!($right),
                        r_val
                    );
                    return Err($crate::KernelTestError::Assert);
                }
            }
        }
    };
}

/// Same as `assert_ne!` but returns a [KernelTestError] instead of `panic`
#[macro_export]
macro_rules! t_assert_ne {
    ($left:expr, $right:expr) => {
        match (&$left, &$right) {
            (l_val, r_val) => {
                if l_val == r_val {
                    log::error!(
                        target: "TEST",
                        "at {}\nAssertion left != right failed\nleft: {} = {:?}\nright: {} = {:?}",
                        $crate::location_str!(),
                        $crate::__macro_internals::stringify!($left),
                        l_val,
                        $crate::__macro_internals::stringify!($right),
                        r_val
                    );
                    return Err($crate::KernelTestError::Assert);
                }
            }
        }
    };
    ($left:expr, $right:expr, $($arg:tt)+) => {
        match (&$left, &$right) {
            (l_val, r_val) => {
                if l_val == r_val {
                    log::error!(
                        target: "TEST",
                        "at {}\nAssertion left != right failed: {}\nleft: {} = {:?}\nright: {} = {:?}",
                        $crate::location_str!(),
                        $crate::__macro_internals::format_args!($($arg)+),
                        $crate::__macro_internals::stringify!($left),
                        l_val,
                        $crate::__macro_internals::stringify!($right),
                        r_val
                    );
                    return Err($crate::KernelTestError::Assert);
                }
            }
        }
    };
}

/// Same as `assert_matches!` but returns a [KernelTestError] instead of `panic`
#[macro_export]
macro_rules! t_assert_matches {
    ($left:expr, $(|)? $( $pattern:pat_param )|+ $( if $guard: expr )? $(,)?) => {
        match $left {
            $( $pattern )|+ $( if $guard )? => {}
            ref expr_val => {
                log::error!(
                    target: "TEST",
                    "at {}\nAssertion expr matchs pattern failed\nexpr: {} = {:?}\npattern: {}",
                    $crate::location_str!(),
                    $crate::__macro_internals::stringify!($left),
                    expr_val,
                    $crate::__macro_internals::stringify!($($pattern)|+ $(if $guard)?),
                );
                return Err($crate::KernelTestError::Assert);
            }
        }
    };
    ($left:expr, $(|)? $( $pattern:pat_param )|+ $( if $guard: expr )?, $($arg:tt)+) => {
        match $left {
            $( $pattern )|+ $( if $guard )? => {}
            ref expr_val => {
                log::error!(
                    target: "TEST",
                    "at {}\nAssertion expr matchs pattern failed: {}\nexpr: {} = {:?}\npattern: {}",
                    $crate::location_str!(),
                    $crate::__macro_internals::format_args!($($arg)+),
                    $crate::__macro_internals::stringify!($left),
                    expr_val,
                    $crate::__macro_internals::stringify!($($pattern)|+ $(if $guard)?),
                );
                return Err($crate::KernelTestError::Assert);
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! location_str {
    () => {
        $crate::__macro_internals::concat!(
            $crate::__macro_internals::file!(),
            ":",
            $crate::__macro_internals::line!(),
            ":",
            $crate::__macro_internals::column!()
        )
    };
}

#[doc(hidden)]
pub mod __macro_internals {
    pub use core::column;
    pub use core::concat;
    pub use core::file;
    pub use core::format_args;
    pub use core::line;
    pub use core::stringify;

    pub enum AssertKind {
        Eq,
        Ne,
        // TODO matches?
    }
}
