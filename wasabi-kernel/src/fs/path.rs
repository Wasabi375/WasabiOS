//! Path and PathBuf abstractions

use alloc::boxed::Box;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Path<'a> {
    path: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBuf {
    path: Box<str>,
}
