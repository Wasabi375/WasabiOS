//! Module containing data/access to core local structures
//!
//! This provides the [locals!] macro as well as [CoreLocals] struct
//! which can be used to access per core data.

use crate::{
    cpu::{self, apic::Apic, cpuid},
    locals,
    prelude::TicketLock,
};
use alloc::boxed::Box;
use core::{
    arch::asm,
    hint::spin_loop,
    sync::atomic::{AtomicU64, AtomicU8, Ordering},
};
use log::{debug, trace};
use shared::{lockcell::InterruptState, types::CoreId};
use x86_64::VirtAddr;

/// A counter used to assign the core id for each core.
/// Each core calls [AtomicU8::fetch_add] to get it's id and automatically increment
/// it for the next core, ensuring ids are unique.
static CORE_ID_COUNTER: AtomicU8 = AtomicU8::new(0);

/// an array with the [VirtAddr]s for each [CoreLocals] indexed by the core id.
static mut CORE_LOCALS_VADDRS: [VirtAddr; 255] = [VirtAddr::zero(); 255];

/// A [CoreLocals] instance used during the boot process.
static mut BOOT_CORE_LOCALS: CoreLocals = CoreLocals::empty();

/// An reference counter that automatically decrements using a [AutoRefCounterGuard].
#[derive(Debug)]
pub struct AutoRefCounter(AtomicU64);

impl AutoRefCounter {
    /// creates a new [AutoRefCounter]
    pub const fn new(init: u64) -> Self {
        Self(AtomicU64::new(init))
    }

    /// returns the count
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    /// increments the count and returns a [AutoRefCounterGuard], which will
    /// decrement the count on [Drop].
    pub fn increment(&self) -> AutoRefCounterGuard<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        AutoRefCounterGuard(self)
    }
}

/// Guard struct which will decrement the count of the associated [AutoRefCounter].
pub struct AutoRefCounterGuard<'a>(&'a AutoRefCounter);

impl<'a> Drop for AutoRefCounterGuard<'a> {
    fn drop(&mut self) {
        (self.0).0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// This struct contains a number of core local entries.
///
/// There is exactly 1 instance of this per core, that has run through the boot sequence,
/// meaning that the core first called [core_boot] and later [init].
///
/// There is also an additional instance that is used after [core_boot] is finished
/// and before [init] was called.
#[repr(C)]
pub struct CoreLocals {
    /// The virtual address of this [CoreLocals] struct. This is used in combination with
    /// `GS` segment to load this struct.
    // NOTE: this must be the first entry in the struct, in order to load
    // the core locals from gs
    virt_addr: VirtAddr,

    /// lock used to access the critical boot section.
    ///
    /// This lock is taken when [core_boot] is called and released at the end of [init].
    boot_lock: AtomicU8,

    /// A unique id of this core. Each core has sequential ids starting from 0 and ending
    /// at [get_max_core_id].
    pub core_id: CoreId,
    /// The core local apic id. This is assigned by the hardware and is not necessarially
    /// equal to the core id.
    pub apic_id: CoreId,

    /// Current depth of interrupts. Every time an interrupt fires, this is incremented
    /// and decremented once the interrupt is done. Therefore a `count() > 0` implies
    /// that we are currently in an interrupt.
    interrupt_count: AutoRefCounter,

    /// Current depth of expcetions. Every time an expcetion fires, this is incremented
    /// and decremented once the expcetion is done. Therefore a `count() > 0` implies
    /// that we are currently in an expcetion.
    exception_count: AutoRefCounter,

    /// A count of how many times [Self::disable_interrupts] has been called. We only reenable
    /// interrupts once this hits 0. This is decremented in [Self::enable_interrupts].
    interrupts_disable_count: AtomicU64,

    /// A lock holding the local apic. This can be [None] if the apic has not been
    /// initialized.
    pub apic: TicketLock<Option<Apic>>,
}

impl CoreLocals {
    /// creates an empty [CoreLocals] struct.
    const fn empty() -> Self {
        Self {
            virt_addr: VirtAddr::zero(),
            boot_lock: AtomicU8::new(0),
            core_id: CoreId(0),
            apic_id: CoreId(0),
            interrupt_count: AutoRefCounter::new(0),
            exception_count: AutoRefCounter::new(0),
            // interrupts disbale count is 1, because the boot section does not allow
            // for interrupts, after all we have not initialized them.
            interrupts_disable_count: AtomicU64::new(1),

            apic: TicketLock::new(None),
        }
    }

    /// increment the [Self::interrupt_count] and return a guard to decrement it again.
    pub fn inc_interrupt(&self) -> AutoRefCounterGuard<'_> {
        self.interrupt_count.increment()
    }

    /// increment the [Self::exception_count] and return a guard to decrement it again.
    pub fn inc_exception(&self) -> AutoRefCounterGuard<'_> {
        self.exception_count.increment()
    }

    /// returns `true` if this core is currently in an interrupt
    pub fn in_interrupt(&self) -> bool {
        self.interrupt_count.count() > 0
    }

    /// returns `true` if this core is currently in an expcetion
    pub fn in_exception(&self) -> bool {
        self.exception_count.count() > 0
    }

    /// Tries to enable interrupts if possible.
    ///
    /// This will decrement [Self::interrupt_count] and will only enable interrupts
    /// when `[Self::interrupt_count] == 0`.
    ///
    /// Also interrupts will never be enabled if we are currently inside an interrupt.
    /// In that case exiting the interrupt will reenable interrupts.
    ///
    /// # Safety:
    ///
    /// * must only be called once for each call to [CoreLocals::disable_interrupts]
    /// * caller must ensure that interrupts are save
    pub unsafe fn enable_interrupts(&self) {
        let old_disable_count = self.interrupts_disable_count.fetch_sub(1, Ordering::SeqCst);

        // If we're not already in an interrupt, and we decremented the
        // interrupt outstanding to 0, we can actually enable interrupts.
        //
        // Since it's possible interrupts can be enabled when we enter an
        // interrupt, if we acquire a lock in an interrupt and release it it
        // may attempt to re-enable interrupts. Thus, we never allow enabling
        // interrupts from an interrupt handler. This means interrupts will
        // correctly get re-enabled in this case when the IRET loads the old
        // interrupt flag.
        if old_disable_count == 1 && !self.in_interrupt() {
            // Safety: Not currently in an interrupt and outstanding interrupt count
            // is 0
            unsafe {
                cpu::enable_interrupts();
            }
        }
    }

    /// Disable interrupts and increment [Self::interrupt_count]
    ///
    /// # Safety:
    ///
    /// caller must ensure that interrupts can be safely disabled
    pub unsafe fn disable_interrupts(&self) {
        self.interrupts_disable_count.fetch_add(1, Ordering::SeqCst);
        unsafe {
            // Safety: see function safety definition
            cpu::disable_interrupts();
        }
    }
}

/// Starts the core boot process, enabling the `locals!` macro to access the boot locals
/// region.
///
/// This will create a critical section that only 1 CPU can enter at a time and ends
/// when [`init`] is called.
///
/// # Safety
///
/// this function must only be called once per CPU core, at the start of the execution.
pub unsafe fn core_boot() -> CoreId {
    let core_id: CoreId = CORE_ID_COUNTER.fetch_add(1, Ordering::AcqRel).into();

    let boot_core_locals = unsafe { &mut BOOT_CORE_LOCALS };

    unsafe {
        cpu::disable_interrupts();
        cpu::set_gs_base(boot_core_locals as *const CoreLocals as u64);
    }

    while boot_core_locals.boot_lock.load(Ordering::SeqCst) != core_id.0 {
        spin_loop();
    }

    boot_core_locals.core_id = core_id;
    boot_core_locals.virt_addr = VirtAddr::from_ptr(boot_core_locals);

    assert_eq!(boot_core_locals.interrupt_count.count(), 0);
    assert_eq!(boot_core_locals.exception_count.count(), 0);
    assert_eq!(
        boot_core_locals
            .interrupts_disable_count
            .load(Ordering::Relaxed),
        1
    );

    core_id
}

/// Ends the core boot process. After this call `locals!` will return a final, heap backed
/// core local memory region.
///
/// # Safety
///
/// this function must only be called once per CPU core, after [`core_boot`] was executed
/// and also after memory and logging was initialized.
pub unsafe fn init(core_id: CoreId) {
    let apic_id = cpuid::apic_id().into();

    let mut core_local = Box::new(CoreLocals {
        virt_addr: VirtAddr::zero(),
        boot_lock: AtomicU8::new(core_id.0),
        core_id,
        apic_id,
        interrupt_count: AutoRefCounter::new(0),
        exception_count: AutoRefCounter::new(0),
        interrupts_disable_count: AtomicU64::new(1),
        apic: TicketLock::new(None),
    });

    core_local.virt_addr = VirtAddr::from_ptr(core_local.as_ref());
    debug!(
        "Core {}: Core Locals initialized from boot locals",
        core_id.0
    );

    unsafe {
        // safety: we are in the kernel boot process and we only access our own
        // core's data
        assert!(CORE_LOCALS_VADDRS[core_id.0 as usize].is_null());
        CORE_LOCALS_VADDRS[core_id.0 as usize] = core_local.virt_addr;
    }

    // set gs base to point to this core_local. That way we can use the core!
    // macro to access the core_locals.
    cpu::set_gs_base(core_local.virt_addr.as_u64());

    unsafe {
        // exit the critical boot section and let next core enter
        // safety: at this point we are still in the boot process so we still
        // have unique access to [BOOT_CORE_LOCALS]
        BOOT_CORE_LOCALS.boot_lock.fetch_add(1, Ordering::SeqCst);
    }

    // don't drop core_local, we want it to life forever in static memory.
    // However we can't use a static variable, because we need 1 per core
    core::mem::forget(core_local);
    trace!("Core {}: locals init done", core_id.0);
}

/// A ZST used to access the interrupt state of this core.
///
/// See [InterruptState]
pub struct CoreInterruptState;

impl InterruptState for CoreInterruptState {
    fn in_interrupt() -> bool {
        locals!().in_interrupt()
    }

    fn in_exception() -> bool {
        locals!().in_exception()
    }

    fn core_id() -> CoreId {
        locals!().core_id
    }

    unsafe fn enter_lock() {
        unsafe {
            // safety: disbaling interrupts is ok for locked critical sections
            locals!().disable_interrupts();
        }
    }

    unsafe fn exit_lock() {
        unsafe {
            // safety: only called once, when a lock-guard is dropped
            locals!().enable_interrupts();
        }
    }
}

/// returns the core id of the last core to check in.
/// This means that this can still grow, if not all cores are initialized
pub fn get_max_core_id() -> u8 {
    CORE_ID_COUNTER.load(Ordering::Acquire)
}

/// Returns this' core [CoreLocals] struct.
///
/// # Safety
///
/// This assumes that `GS` segement was initialized by [init] to point to
/// the [CoreLocals] struct for this core.
pub unsafe fn get_core_locals() -> &'static CoreLocals {
    unsafe {
        let ptr: usize;
        // Safety: we assume that gs contains the vaddr of this cores
        // [CoreLocals], which starts with it's own address, therefor we
        // can access the [CoreLocals] vaddr using `gs:[0]`
        asm! {
            "mov {0}, gs:[0]",
            out(reg) ptr
        }

        &*(ptr as *const CoreLocals)
    }
}

/// A macro wrapper around [get_core_locals] returning this' core [CoreLocals] struct.
///
/// # Safety
///
/// This assumes that `GS` segement was initialized by [init] to point to
/// the [CoreLocals] struct for this core.
///
/// This macro includes the necessary unsafe block to allow calling this from safe
/// rust, but it's still unsafe before [core_boot] and/or [init] have been called.
#[macro_export]
macro_rules! locals {
    () => {{
        #[allow(unused_unsafe)]
        let locals = unsafe { $crate::core_local::get_core_locals() };

        locals
    }};
}
