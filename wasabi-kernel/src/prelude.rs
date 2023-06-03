use crate::core_local::CoreInterruptState;

pub use shared::lockcell::{LockCell, LockCellGuard};

pub type SpinLock<T> = shared::lockcell::SpinLock<T, CoreInterruptState>;
pub type TicketLock<T> = shared::lockcell::TicketLock<T, CoreInterruptState>;

pub mod types {
    pub use shared::types::*;
}
