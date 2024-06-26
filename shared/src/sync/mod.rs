//! Module containing syncronization primitives
//!
use crate::types::CoreId;

pub mod barrier;
pub mod lockcell;

/// Trait that allows access to OS-level constructs defining basic information
/// about the current processor.
pub trait CoreInfo: 'static {
    /// Gets the ID of the running core. It's required that this core ID is
    /// unique to the core.
    fn core_id() -> CoreId;

    /// Returns `true` if the current processor is the bootstrap processor.
    fn is_bsp() -> bool;

    /// Returns `true` if the current processor is an application processor.
    ///
    /// This is `true`  if [Self::is_bsp] is `false`
    fn is_ap() -> bool {
        !Self::is_bsp()
    }
}

/// Trait that allows access to OS-level constructs defining interrupt state,
/// exception state, unique core IDs, and enter/exit lock (for interrupt
/// disabling and enabling) primitives.
pub trait InterruptState: CoreInfo + 'static {
    /// Returns `true` if we're currently in an interrupt
    fn in_interrupt() -> bool;

    /// Returns `true` if we're currently in an exception. Which indicates that
    /// a lock cannot be held as we may have pre-empted a non-preemptable lock
    fn in_exception() -> bool;

    /// Signal the kernel that a lock was taken. If `disable_interrupts` the
    /// lock does not support being interrupted and therefor we must disable
    /// interrupts. This is also a prequisite for a lock to be taken within an
    /// interrupt.
    ///
    /// # Safety:
    ///
    /// * Caller must call [InterruptState::exit_lock] exactly once with the
    ///     same parameter for `enable_interrupts`
    /// * If `disable_interrupts` caller must ensure that interrupts can be
    ///     disabled safely
    unsafe fn enter_lock(disable_interrupts: bool);

    /// Signal the kernel that a lock was released. If `enable_interrupts` the
    /// kernel will reenable interrupts if possible.
    ///
    /// # Safety:
    ///
    /// * caller must ensure that this function is called exactly once per invocation
    ///     of [InterruptState::enter_lock] with the same parameter.
    unsafe fn exit_lock(enable_interrupts: bool);

    /// returns the instance of this interrupt state. This should always be a zst.
    fn instance() -> Self;
}
