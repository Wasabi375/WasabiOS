//! re-export of types defined elsewhere for better access

use crate::core_local::CoreInterruptState;

pub use shared::lockcell::{LockCell, LockCellGuard};

/// a [SpinLock](shared::lockcell::SpinLock) with the [CoreInterruptState]
pub type SpinLock<T> = shared::lockcell::SpinLock<T, CoreInterruptState>;
/// a [TicketLoc](shared::lockcell::TicketLock) with the [CoreInterruptState]
pub type TicketLock<T> = shared::lockcell::TicketLock<T, CoreInterruptState>;

/// Some common type definitions, that don't fit within a specific module
pub mod types {
    pub use shared::types::*;
}
