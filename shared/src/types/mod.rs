//! common type definitions

mod coreid;
mod time;

use core::cell::UnsafeCell;
use core::marker::PhantomData;

pub use coreid::CoreId;
pub use time::Duration;
pub use time::TscDuration;
pub use time::TscTimestamp;

/// Marker type to ensure a type is not [Send]
pub type NotSend = PhantomData<*const ()>;

/// Marker type to ensure a type is not [Sync]
pub type NotSync = PhantomData<UnsafeCell<()>>;

#[cfg(doctest)]
pub mod doc_test {
    /// ```compile_fail,E02777
    /// use shared::types::*;
    /// use shared::types::doc_test::*;
    /// test_is_send::<NotSend>();
    /// ```
    #[allow(unused)]
    pub fn test_not_send() {}

    /// ```compile_fail,E02777
    /// use shared::types::*;
    /// use shared::types::doc_test::*;
    /// test_is_sync::<NotSync>();
    /// ```
    #[allow(unused)]
    pub fn test_not_sync() {}

    #[allow(unused, missing_docs)]
    pub fn test_is_send<T: Send>() {}
    #[allow(unused, missing_docs)]
    pub fn test_is_sync<T: Sync>() {}
}
