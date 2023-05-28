#![no_std]
#![feature(negative_impls)]
#![feature(maybe_uninit_uninit_array, maybe_uninit_slice)]

pub mod lockcell;
pub mod primitive_enum;
pub mod rangeset;

#[allow(dead_code)]
#[allow(non_snake_case)]
pub mod sizes {

    pub const fn KiB(n: usize) -> usize {
        1024 * n
    }

    pub const fn MiB(n: usize) -> usize {
        1024 * 1024 * n
    }

    pub const fn GiB(n: usize) -> usize {
        1024 * 1024 * 1024 * n
    }

    mod drive {
        pub const fn KB(n: u64) -> u64 {
            1000 * n
        }

        pub const fn MB(n: u64) -> u64 {
            1000 * 1000 * n
        }

        pub const fn GB(n: u64) -> u64 {
            1000 * 1000 * 1000 * n
        }
    }
}
