#![no_std]
#![feature(
    negative_impls,
    maybe_uninit_uninit_array,
    maybe_uninit_slice,
    let_chains
)]

pub mod lockcell;
pub mod primitive_enum;
pub mod rangeset;

/// common type definitions
pub mod types {
    use core::ops::{Deref, DerefMut};

    /// contains the id for a given core
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct CoreId(pub u8);

    impl CoreId {
        /// Whether this core is the bootstrap core used for initialization of
        /// global systems
        pub fn is_bsc(&self) -> bool {
            self.0 == 0
        }
    }

    impl Deref for CoreId {
        type Target = u8;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl DerefMut for CoreId {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }

    impl From<u8> for CoreId {
        fn from(value: u8) -> Self {
            Self(value)
        }
    }

    impl Into<u8> for CoreId {
        fn into(self) -> u8 {
            self.0
        }
    }
}

/// size calulation utilities
///
/// TODO rework as enum?
#[allow(dead_code)]
#[allow(non_snake_case)]
pub mod sizes {

    /// calulates KiB in bytes
    pub const fn KiB(n: usize) -> usize {
        1024 * n
    }

    /// calulates MiB in bytes
    pub const fn MiB(n: usize) -> usize {
        1024 * 1024 * n
    }

    /// calulates GiB in bytes
    pub const fn GiB(n: usize) -> usize {
        1024 * 1024 * 1024 * n
    }
}
