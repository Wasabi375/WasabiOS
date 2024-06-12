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

pub mod lockcell;
pub mod primitive_enum;
pub mod rangeset;

#[cfg(feature = "alloc")]
pub mod reforbox;

pub trait CoreInfo: 'static {
    /// Gets the ID of the running core. It's required that this core ID is
    /// unique to the core.
    fn core_id() -> types::CoreId;

    /// Returns `true` if the current processor is the bootstrap processor.
    fn is_bsp() -> bool;

    /// Returns `true` if the current processor is an application processor.
    ///
    /// This is `true`  if [Self::is_bsp] is `false`
    fn is_ap() -> bool {
        !Self::is_bsp()
    }
}

/// common type definitions
pub mod types {
    use core::{
        fmt,
        ops::{Deref, DerefMut},
    };

    /// contains the id for a given core
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct CoreId(pub u8);

    impl CoreId {
        /// Whether this core is used as the bootstrap processor used for initialization of
        /// global systems
        pub fn is_bsp(&self) -> bool {
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

    impl fmt::Display for CoreId {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fmt(f)
        }
    }
}

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
