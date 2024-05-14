//! Library providing #[test] like functionaility for my kernel

#![no_std]
#![feature(error_in_core)]
#![warn(missing_docs, rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

extern crate alloc;

use core::error::Error;

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

/// Extensio for the Result type useful for tests
pub trait TestResultExt<T> {
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

impl<T, E: Error> TestResultExt<T> for Result<T, E> {
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
