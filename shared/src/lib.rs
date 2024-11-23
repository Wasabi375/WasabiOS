//! Implementation shared by all kernel packages
#![no_std]
#![feature(
    box_into_inner,
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
