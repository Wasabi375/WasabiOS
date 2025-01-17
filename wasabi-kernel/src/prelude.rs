//! re-export of types defined elsewhere for better access

pub use shared::sync::lockcell::{LockCell, LockCellGuard};

use crate::core_local::CoreInterruptState;

/// a [UnsafeTicketLock](shared::sync::lockcell::UnsafeTicketLock) with the [CoreInterruptState]
pub type UnsafeTicketLock = shared::sync::lockcell::UnsafeTicketLock<CoreInterruptState>;

/// a [TicketLock](shared::sync::lockcell::TicketLock) with the [CoreInterruptState]
pub type TicketLock<T> = shared::sync::lockcell::TicketLock<T, CoreInterruptState>;

/// a [ReadWriteCell](shared::sync::lockcell::ReadWriteCell) with the [CoreInterruptState]
pub type ReadWriteCell<T> = shared::sync::lockcell::ReadWriteCell<T, CoreInterruptState>;

/// a [UnwrapTicketLock](shared::sync::lockcell::UnwrapTicketLock) with the [CoreInterruptState]
pub type UnwrapTicketLock<T> = shared::sync::lockcell::UnwrapTicketLock<T, CoreInterruptState>;

/// a [DataBarrier](shared::sync::barrier::DataBarrier) with the [CoreInterruptState]
pub type DataBarrier<T> = shared::sync::barrier::DataBarrier<T, CoreInterruptState>;

/// a [SingleCoreLock](shared::sync::single_core_lock::SingleCoreLock) with the
/// [CoreInterruptState]
pub type SingleCoreLock = shared::sync::single_core_lock::SingleCoreLock<CoreInterruptState>;

/// Some common type definitions, that don't fit within a specific module
pub mod types {
    pub use shared::types::*;
}
