//! re-export of types defined elsewhere for better access

use crate::core_local::CoreInterruptState;

pub use shared::lockcell::{LockCell, LockCellGuard};

/// a [SpinLock](shared::lockcell::SpinLock) with the [CoreInterruptState]
pub type SpinLock<T> = shared::lockcell::SpinLock<T, CoreInterruptState>;

/// a [TicketLoc](shared::lockcell::TicketLock) with the [CoreInterruptState]
pub type TicketLock<T> = shared::lockcell::TicketLock<T, CoreInterruptState>;

/// a [UnwrapSpinLock](shared::lockcell::UnwrapSpinLock) with the [CoreInterruptState]
pub type UnwrapSpinLock<T> = shared::lockcell::UnwrapSpinLock<T, CoreInterruptState>;

/// a [UnwrapTicketLock](shared::lockcell::UnwrapTicketLock) with the [CoreInterruptState]
pub type UnwrapTicketLock<T> = shared::lockcell::UnwrapTicketLock<T, CoreInterruptState>;

/// Some common type definitions, that don't fit within a specific module
pub mod types {
    pub use shared::types::*;
}
