use core::{
    arch::asm,
    hint::spin_loop,
    sync::atomic::{AtomicU64, AtomicU8, Ordering},
};

use crate::{
    cpu::{self, apic::Apic, cpuid},
    locals,
    prelude::TicketLock,
};
use alloc::boxed::Box;
use log::debug;
use shared::{lockcell::InterruptState, types::CoreId};
use x86_64::VirtAddr;

static CORE_ID_COUNTER: AtomicU8 = AtomicU8::new(0);
static mut CORE_LOCALS_VADDRS: [VirtAddr; 255] = [VirtAddr::zero(); 255];

static mut BOOT_CORE_LOCALS: CoreLocals = CoreLocals::empty();

#[derive(Debug)]
pub struct AutoRefCounter(AtomicU64);

impl AutoRefCounter {
    pub const fn new(init: u64) -> Self {
        Self(AtomicU64::new(init))
    }

    pub fn count(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    pub fn increment(&self) -> AutoRefCounterGuard<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        AutoRefCounterGuard(self)
    }
}

pub struct AutoRefCounterGuard<'a>(&'a AutoRefCounter);

impl<'a> Drop for AutoRefCounterGuard<'a> {
    fn drop(&mut self) {
        (self.0).0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[repr(C)]
pub struct CoreLocals {
    // NOTE: this must be the first entry in the struct, in order to load
    // the core locals from gs
    virt_addr: VirtAddr,

    /// lock used to access the critical boot section.
    /// This should not be used. It is only public for a sanity assert in the
    /// [`locals!`] macro.
    #[doc(hidden)]
    pub __boot_lock: AtomicU8,

    pub core_id: CoreId,
    pub apic_id: CoreId, // TODO create ApicId struct instead?

    interrupt_count: AutoRefCounter,
    exception_count: AutoRefCounter,
    interrupts_disable_count: AtomicU64,

    pub apic: TicketLock<Option<Apic>>,
}

impl CoreLocals {
    const fn empty() -> Self {
        Self {
            virt_addr: VirtAddr::zero(),
            __boot_lock: AtomicU8::new(0),
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

    pub fn inc_interrupt(&self) -> AutoRefCounterGuard<'_> {
        self.interrupt_count.increment()
    }

    pub fn inc_exception(&self) -> AutoRefCounterGuard<'_> {
        self.exception_count.increment()
    }

    pub fn in_interrupt(&self) -> bool {
        self.interrupt_count.count() > 0
    }

    pub fn in_exception(&self) -> bool {
        self.exception_count.count() > 0
    }

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
            unsafe {
                cpu::enable_interrupts();
            }
        }
    }

    pub unsafe fn disable_interrupts(&self) {
        self.interrupts_disable_count.fetch_add(1, Ordering::SeqCst);
        unsafe {
            cpu::disable_interrupts();
        }
    }
}

/// Starts the core boot process, enabling the `locals!` macro to access the boot locals
/// region.
///
/// # Safety
///
/// this function must only be called once per CPU core, at the start of the execution.
/// This will create a critical section that only 1 CPU can enter at a time and ends
/// when [`init`] is called.
pub unsafe fn core_boot() -> CoreId {
    let core_id: CoreId = CORE_ID_COUNTER.fetch_add(1, Ordering::AcqRel).into();

    unsafe {
        cpu::disable_interrupts();
        cpu::set_gs_base(&BOOT_CORE_LOCALS as *const CoreLocals as u64);

        while BOOT_CORE_LOCALS.__boot_lock.load(Ordering::SeqCst) != core_id.0 {
            spin_loop();
        }

        BOOT_CORE_LOCALS.core_id = core_id;
        BOOT_CORE_LOCALS.virt_addr = VirtAddr::from_ptr(&BOOT_CORE_LOCALS);

        assert_eq!(BOOT_CORE_LOCALS.interrupt_count.count(), 0);
        assert_eq!(BOOT_CORE_LOCALS.exception_count.count(), 0);
        assert_eq!(
            BOOT_CORE_LOCALS
                .interrupts_disable_count
                .load(Ordering::Relaxed),
            1
        );
    }

    core_id
}

/// Ends the core boot process. After this call `locals!` will return a final, heap backed
/// core local memory region.
///
/// # Safety
///
/// this function must only be called once per CPU core, after [`core_boot`] was executed
/// and also after memory and logging was set up.
pub unsafe fn init(core_id: CoreId) {
    let apic_id = cpuid::apic_id().into();

    let core_local = Box::new(CoreLocals {
        virt_addr: VirtAddr::zero(),
        __boot_lock: AtomicU8::new(core_id.0),
        core_id,
        apic_id,
        interrupt_count: AutoRefCounter::new(0),
        exception_count: AutoRefCounter::new(0),
        interrupts_disable_count: AtomicU64::new(1),
        apic: TicketLock::new(None),
    });

    core_local.virt_addr = VirtAddr::from_ptr(core_local);
    debug!("Core Locals initialized from boot locals");

    unsafe {
        assert!(CORE_LOCALS_VADDRS[core_id.0 as usize].is_null());
        CORE_LOCALS_VADDRS[core_id.0 as usize] = core_local.virt_addr;
    }

    unsafe {
        // set gs base to point to this core_local. That way we can use the core!
        // macro to access the core_locals.
        cpu::set_gs_base(core_local.virt_addr.as_u64());

        // exit the critical boot section and let next core enter
        BOOT_CORE_LOCALS.__boot_lock.fetch_add(1, Ordering::SeqCst);
    }
}

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

    fn enter_lock() {
        unsafe {
            locals!().disable_interrupts();
        }
    }

    fn exit_lock() {
        unsafe {
            locals!().enable_interrupts();
        }
    }
}

pub struct BootSafeCoreInterruptState;

impl InterruptState for BootSafeCoreInterruptState {
    fn in_interrupt() -> bool {
        if unsafe { core_locals_initialized() } {
            locals!().in_interrupt()
        } else {
            false
        }
    }

    fn in_exception() -> bool {
        if unsafe { core_locals_initialized() } {
            locals!().in_exception()
        } else {
            false
        }
    }

    fn core_id() -> CoreId {
        if unsafe { core_locals_initialized() } {
            locals!().core_id
        } else {
            CoreId(!0)
        }
    }

    fn enter_lock() {
        todo!()
    }

    fn exit_lock() {
        todo!()
    }
}

/// returns the core id of the last core to check in.
/// This means that this can still grow, if not all cores are initialized
pub fn get_max_core_id() -> u8 {
    CORE_ID_COUNTER.load(Ordering::Acquire)
}

pub fn get_core_locals() -> &'static CoreLocals {
    unsafe {
        let ptr: usize;
        asm! {
            "mov {0}, gs:[0]",
            out(reg) ptr
        }

        &*(ptr as *const CoreLocals)
    }
}

pub unsafe fn core_locals_initialized() -> bool {
    cpu::gs_base() != 0
}

#[macro_export]
macro_rules! locals {
    () => {{
        use core::sync::atomic::Ordering;
        let locals = $crate::core_local::get_core_locals();

        let lock = locals.__boot_lock.load(Ordering::Relaxed);
        assert_eq!(lock, locals.core_id.0);
        locals
    }};
}
