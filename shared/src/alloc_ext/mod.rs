//! Additional traits and structs that depend on alloc

#![allow(missing_docs)]

pub mod reforbox;
mod single_arc;

pub use single_arc::SingleArc;
pub use single_arc::Strong;
pub use single_arc::Weak;
pub use single_arc::WeakSingleArc;
