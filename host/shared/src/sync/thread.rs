//! Threading primitives
//!
//! It is not always possible to just use normal threads when working
//! with kernel libraries as they expect threading to follow the kernel
//! internal structures.
//!
//! This module provides a mapping between ther kernel multithreading
//! and host threading

// TODO
