//! re-export of types defined elsewhere for better access

pub use shared::sync::lockcell::{LockCell, LockCellGuard};

use crate::core_local::CoreInterruptState;

/// a [TicketLoc](shared::sync::lockcell::TicketLock) with the [CoreInterruptState]
pub type TicketLock<T> = shared::sync::lockcell::TicketLock<T, CoreInterruptState>;

/// a [ReadWriteCell](shared::sync::lockcell::ReadWriteCell) with the [CoreInterruptState]
pub type ReadWriteCell<T> = shared::sync::lockcell::ReadWriteCell<T, CoreInterruptState>;

/// a [UnwrapTicketLock](shared::sync::lockcell::UnwrapTicketLock) with the [CoreInterruptState]
pub type UnwrapTicketLock<T> = shared::sync::lockcell::UnwrapTicketLock<T, CoreInterruptState>;

/// Some common type definitions, that don't fit within a specific module
pub mod types {
    pub use shared::types::*;
}
