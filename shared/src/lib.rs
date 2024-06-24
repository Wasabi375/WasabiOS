#![no_std]
#![feature(
    negative_impls,
    maybe_uninit_uninit_array,
    maybe_uninit_slice,
    let_chains
)]
#![cfg_attr(feature = "alloc", feature(box_into_inner))]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod cpu;
pub mod primitive_enum;
pub mod rangeset;
pub mod sync;

#[cfg(feature = "alloc")]
pub mod reforbox;

pub mod types;

#[macro_export]
macro_rules! KiB {
    ($v:expr) => {
        $v * 1024
    };
}

#[macro_export]
macro_rules! MiB {
    ($v:expr) => {
        $v * 1024 * 1024
    };
}

#[macro_export]
macro_rules! GiB {
    ($v:expr) => {
        $v * 1024 * 1024 * 1024
    };
}
