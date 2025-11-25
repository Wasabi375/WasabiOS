//! Module containing syncronization primitives
//!
use crate::types::CoreId;

pub mod barrier;
pub mod lockcell;
pub mod single_core_lock;

/// Trait that allows access to OS-level constructs defining basic information
/// about the current processor.
pub trait CoreInfo: 'static + Send + Sync {
    /// Gets the ID of the running core. It's required that this core ID is
    /// unique to the core.
    fn core_id(&self) -> CoreId;

    /// Returns `true` if the current processor is the bootstrap processor.
    fn is_bsp(&self) -> bool;

    /// Returns `true` if the current processor is an application processor.
    ///
    /// This is `true`  if [Self::is_bsp] is `false`
    fn is_ap(&self) -> bool {
        !self.is_bsp()
    }

    /// Returns `true` if a "task system" is running on the processor
    ///
    /// This result of this value can only switch from `false` to `true`.
    /// Therefor it is save to assume that once this returns `true` all future
    /// calls to this function will always return `true`.
    ///
    /// This function will not have any side-effects.
    fn task_system_is_init(&self) -> bool;

    /// Writes the debug name and or taks handle to the writer
    ///
    /// This is used to print debug information and can be a Nop.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the TaskSystem is initialized. See [CoreInfo::task_system_is_init]
    unsafe fn write_current_task_name(
        &self,
        writer: &mut dyn core::fmt::Write,
    ) -> Result<(), core::fmt::Error>;

    /// Returns `true` if the [CoreInfo] is fully initialized.
    ///
    /// If set to `false` the core is in it's early boot state and not all functionallity
    /// is available.
    fn is_initialized(&self) -> bool;

    /// returns the instance of this interrupt state. This should always be a zst.
    fn instance() -> Self
    where
        Self: Sized;

    /// Gets the ID of the running core. It's required that this core ID is
    /// unique to the core.
    #[inline(always)]
    fn s_core_id() -> CoreId
    where
        Self: Sized,
    {
        Self::instance().core_id()
    }

    /// Returns `true` if the current processor is the bootstrap processor.
    #[inline(always)]
    fn s_is_bsp() -> bool
    where
        Self: Sized,
    {
        Self::instance().is_bsp()
    }

    /// Returns `true` if the current processor is an application processor.
    ///
    /// This is `true`  if [Self::is_bsp] is `false`
    #[inline(always)]
    fn s_is_ap() -> bool
    where
        Self: Sized,
    {
        Self::instance().is_ap()
    }

    /// Returns the maximum number of cores running.
    fn max_core_count() -> u8
    where
        Self: Sized;
}

/// Trait that allows access to OS-level constructs defining interrupt state,
/// exception state, unique core IDs, and enter/exit lock (for interrupt
/// disabling and enabling) primitives.
pub trait InterruptState: CoreInfo + 'static {
    /// Returns `true` if we're currently in an interrupt
    fn in_interrupt(&self) -> bool;

    /// Returns `true` if we're currently in an exception. Which indicates that
    /// a lock cannot be held as we may have pre-empted a non-preemptable lock
    fn in_exception(&self) -> bool;

    /// Signal the kernel that a lock was taken. If `disable_interrupts` the
    /// lock does not support being interrupted and therefor we must disable
    /// interrupts. This is also a prequisite for a lock to be taken within an
    /// interrupt.
    ///
    /// # Safety
    ///
    /// * Caller must call [InterruptState::exit_lock] exactly once with the
    ///   same parameter for `enable_interrupts`
    /// * If `disable_interrupts` caller must ensure that interrupts can be
    ///   disabled safely
    unsafe fn enter_lock(&self, disable_interrupts: bool);

    /// Signal the kernel that a lock was released. If `enable_interrupts` the
    /// kernel will reenable interrupts if possible.
    ///
    /// # Safety
    ///
    /// * caller must ensure that this function is called exactly once per invocation
    ///   of [InterruptState::enter_lock] with the same parameter.
    unsafe fn exit_lock(&self, enable_interrupts: bool);

    /// Returns `true` if we're currently in an interrupt
    fn s_in_interrupt() -> bool
    where
        Self: Sized,
    {
        Self::instance().in_interrupt()
    }

    /// Returns `true` if we're currently in an exception. Which indicates that
    /// a lock cannot be held as we may have pre-empted a non-preemptable lock
    fn s_in_exception() -> bool
    where
        Self: Sized,
    {
        Self::instance().in_exception()
    }

    /// Signal the kernel that a lock was taken. If `disable_interrupts` the
    /// lock does not support being interrupted and therefor we must disable
    /// interrupts. This is also a prequisite for a lock to be taken within an
    /// interrupt.
    ///
    /// # Safety
    ///
    /// * Caller must call [InterruptState::exit_lock] exactly once with the
    ///   same parameter for `enable_interrupts`
    /// * If `disable_interrupts` caller must ensure that interrupts can be
    ///   disabled safely
    unsafe fn s_enter_lock(disable_interrupts: bool)
    where
        Self: Sized,
    {
        unsafe { Self::instance().enter_lock(disable_interrupts) }
    }

    /// Signal the kernel that a lock was released. If `enable_interrupts` the
    /// kernel will reenable interrupts if possible.
    ///
    /// # Safety
    ///
    /// * caller must ensure that this function is called exactly once per invocation
    ///   of [InterruptState::enter_lock] with the same parameter.
    unsafe fn s_exit_lock(enable_interrupts: bool)
    where
        Self: Sized,
    {
        unsafe { Self::instance().exit_lock(enable_interrupts) }
    }
}
