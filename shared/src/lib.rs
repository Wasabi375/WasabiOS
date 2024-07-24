//! Implementation shared by all kernel packages
#![no_std]
#![feature(
    negative_impls,
    maybe_uninit_uninit_array,
    maybe_uninit_slice,
    let_chains,
    downcast_unchecked
)]
#![cfg_attr(feature = "alloc", feature(box_into_inner))]
#![warn(missing_docs)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod cpu;
pub mod math;
pub mod primitive_enum;
pub mod rangeset;
pub mod sync;
pub mod types;

#[cfg(feature = "alloc")]
pub mod alloc_ext;

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
