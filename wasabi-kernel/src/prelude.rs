//! re-export of types defined elsewhere for better access

use crate::core_local::CoreInterruptState;

pub use shared::lockcell::{LockCell, LockCellGuard};

/// a [TicketLoc](shared::lockcell::TicketLock) with the [CoreInterruptState]
pub type TicketLock<T> = shared::lockcell::TicketLock<T, CoreInterruptState>;

/// a [ReadWriteCell](shared::lockcell::ReadWriteCell) with the [CoreInterruptState]
pub type ReadWriteCell<T> = shared::lockcell::ReadWriteCell<T, CoreInterruptState>;

/// a [UnwrapTicketLock](shared::lockcell::UnwrapTicketLock) with the [CoreInterruptState]
pub type UnwrapTicketLock<T> = shared::lockcell::UnwrapTicketLock<T, CoreInterruptState>;

/// Some common type definitions, that don't fit within a specific module
pub mod types {
    pub use shared::types::*;
}
