//! Structs representing different UEFI GPT records
//!
//! The specifications are available from <https://uefi.org/specifications>
//! Specifically relevant is the UEFI Specification.
//!
//! This is based on UEFI Specifiaction Version 2.10(released Auguast 2022),
//! although I assume that any 2.x version works

pub mod chs;

pub mod protective_mbr;
