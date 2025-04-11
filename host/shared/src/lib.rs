//! Shared implementation and traits used in libs/programs that run
//! on the host system.
//!
//! This can be used for rust tests, but also programs like the wfs fuse-driver.

#![warn(missing_docs)]
#![feature(thread_id_value)]

pub mod sync;

