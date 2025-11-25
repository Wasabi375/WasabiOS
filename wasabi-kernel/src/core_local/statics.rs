//! Kernel static core data structures

#[allow(unused_imports)]
use log::{debug, info, trace, warn};

#[cfg(feature = "test")]
use crate::testing::core_local::TestCoreStatics;

use crate::{
    cpu::{self, apic::Apic, cpuid, gdt::GDTInfo, interrupts::InterruptHandlerState},
    crossbeam_epoch::LocalEpochHandle,
    prelude::UnwrapTicketLock,
    task::TaskSystem,
};
use alloc::boxed::Box;
use core::{
    arch::asm,
    hint::spin_loop,
    ptr::{NonNull, addr_of_mut},
    sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
use shared::types::CoreId;

use super::{AutoRefCounter, AutoRefCounterGuard, CORE_ID_COUNTER, CORE_READY_COUNT};

/// an array with the [NonNull]s for each [CoreStatics] indexed by the core id.
static mut CORE_LOCALS_VADDRS: [Option<NonNull<CoreStatics>>; 255] = [None; 255];

/// A [CoreStatics] instance used during the boot process.
static mut BOOT_CORE_LOCALS: CoreStatics = CoreStatics::empty();

/// lock used to access the critical boot section.
///
/// This lock is taken when [core_boot] is called and released at the end of [init].
static BOOT_LOCK: AtomicU8 = AtomicU8::new(0);

/// This struct contains a number of core local entries.
///
/// There is exactly 1 instance of this per core, that has run through the boot sequence,
/// meaning that the core first called [core_boot] and later [init].
///
/// There is also an additional instance that is used after [core_boot] is finished
/// and before [init] was called.
#[repr(C)]
pub struct CoreStatics {
    /// The virtual address of this [CoreStatics] struct. This is used in combination with
    /// `GS` segment to load this struct.
    // NOTE: this must be the first entry in the struct, in order to load
    // the core locals from gs
    self_ptr: NonNull<CoreStatics>,

    /// A unique id of this core. Each core has sequential ids starting from 0 and ending
    /// at [get_started_core_count].
    pub core_id: CoreId,

    /// The core local apic id. This is assigned by the hardware and is not necessarially
    /// equal to the core id.
    pub apic_id: CoreId,

    /// Current depth of interrupts. Every time an interrupt fires, this is incremented
    /// and decremented once the interrupt is done. Therefore a `count() > 0` implies
    /// that we are currently in an interrupt.
    pub interrupt_count: AutoRefCounter,

    /// Current depth of expcetions. Every time an expcetion fires, this is incremented
    /// and decremented once the expcetion is done. Therefore a `count() > 0` implies
    /// that we are currently in an expcetion.
    pub exception_count: AutoRefCounter,

    /// A count of how many times [Self::disable_interrupts] has been called. We only reenable
    /// interrupts once this hits 0. This is decremented in [Self::enable_interrupts].
    interrupts_disable_count: AtomicU64,

    /// Interrupt state related to handling interrupts
    pub interrupt_state: InterruptHandlerState,

    /// A lock holding the local apic. This can be [None] if the apic has not been
    /// initialized.
    ///
    /// # Safety
    ///
    /// [cpu::apic::init] must be called before this can be used
    pub apic: UnwrapTicketLock<Apic>,

    /// True if panic needs to issue nmi to disable other processors
    pub nmi_on_panic: AtomicBool,

    /// The gdt for this core.
    ///
    /// Also contains memory for TSS.
    ///
    /// # Safety
    ///
    /// [GDTInfo::init_and_load] needs to be called before this is
    /// useable
    pub gdt: GDTInfo,

    /// Set to true after the locals are fully initialized
    pub initialized: bool,

    /// Handle for crossbeam epoch
    pub epoch_handle: LocalEpochHandle,

    /// The task system used for concurency on this core
    pub task_system: UnwrapTicketLock<TaskSystem>,

    /// Core locals used by tests
    #[cfg(feature = "test")]
    pub test_local: TestCoreStatics,
}

impl CoreStatics {
    /// creates an empty [CoreStatics] struct.
    const fn empty() -> Self {
        Self {
            self_ptr: NonNull::dangling(),
            core_id: CoreId(0),
            apic_id: CoreId(0),
            interrupt_count: AutoRefCounter::new(0),
            exception_count: AutoRefCounter::new(0),
            // interrupts disbale count is 1, because the boot section does not allow
            // for interrupts, after all we have not initialized them.
            interrupts_disable_count: AtomicU64::new(1),

            interrupt_state: InterruptHandlerState::new(),

            apic: unsafe { UnwrapTicketLock::new_non_preemtable_uninit() },
            nmi_on_panic: AtomicBool::new(false),

            gdt: GDTInfo::new_uninit(),

            epoch_handle: LocalEpochHandle::new_uninit(),

            task_system: unsafe { UnwrapTicketLock::new_non_preemtable_uninit() },

            initialized: false,

            #[cfg(feature = "test")]
            test_local: TestCoreStatics::new(),
        }
    }

    /// initializes [CoreStatics] for this core from the [BOOT_CORE_LOCALS].
    ///
    /// This creates a fully functional [CoreStatics] with the relevant fields
    /// from the boot copied over.
    ///
    /// # Safety
    ///
    /// This function must only be called once for each core, after [core_boot] and
    /// before [init].
    ///
    /// The caller must ensure that there are no references into [BOOT_CORE_LOCALS]
    /// either via the static ref, the [super::locals] macro or in any other way.
    /// This should be guranteed by the `boot_locals` parameter.
    unsafe fn initialize(boot_locals: &mut Self, core_id: CoreId, apic_id: CoreId) -> Box<Self> {
        trace!("CoreStatic::initialize");

        assert_eq!(boot_locals.interrupt_count.count(), 0);
        assert_eq!(boot_locals.exception_count.count(), 0);
        assert_eq!(
            boot_locals.interrupts_disable_count.load(Ordering::Relaxed),
            1
        );

        let mut reset_boot_locals = CoreStatics::empty();
        // The self ptr must be set for `locals!` to work.
        // See [get_core_statics]
        reset_boot_locals.self_ptr = NonNull::from(unsafe {
            // Safety: we are still during the `core_boot` process and therefore have unique
            // access to BOOT_CORE_LOCALS
            &mut *addr_of_mut!(BOOT_CORE_LOCALS)
        });

        let boot_locals = core::mem::replace(boot_locals, reset_boot_locals);

        let mut core_statics = Box::new(CoreStatics {
            self_ptr: NonNull::dangling(),
            core_id,
            apic_id,
            interrupt_count: AutoRefCounter::new(0),
            exception_count: AutoRefCounter::new(0),
            // interrupts disbale count is 1, because the boot section does not allow
            // for interrupts, after all we have not initialized them.
            interrupts_disable_count: AtomicU64::new(1),

            initialized: true,

            // Move all the setup work that has been done so far, we just need
            // to override some specific data to move to a new self_ptr
            ..boot_locals
        });

        core_statics.self_ptr = NonNull::from(core_statics.as_mut());

        core_statics
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
    /// # Safety
    ///
    /// * must only be called once for each call to [CoreStatics::disable_interrupts]
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
        // interrupt flag as part of the EFLAGS register.
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
    /// # Safety
    ///
    /// caller must ensure that interrupts can be safely disabled
    pub unsafe fn disable_interrupts(&self) {
        self.interrupts_disable_count.fetch_add(1, Ordering::SeqCst);
        unsafe {
            // Safety: see function safety definition
            cpu::disable_interrupts();
        }
    }

    /// returns `true` if interrupts are currently enabled
    ///
    /// this can break if [`cpu::disable_interrupts`] is used instead of
    /// the core local [`CoreStatics::disable_interrupts`].
    ///
    /// This function uses a relaxed load, and should therefor only be used
    /// for diagnostics.
    pub fn interrupts_enabled(&self) -> bool {
        self.interrupts_disable_count.load(Ordering::Relaxed) == 0
    }

    /// returns `true` if this core is used as the bootstrap processor
    pub fn is_bsp(&self) -> bool {
        self.core_id.is_bsp()
    }

    /// returns `true` if this core is an application processor
    ///
    /// this is `true` iff [Self::is_bsp] is `false`
    pub fn is_ap(&self) -> bool {
        !self.is_bsp()
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
/// this functon must only be called once per CPU core, at the start of the execution.
/// Also [`locals!`] does not return a static ref before [`init`] finished.
pub unsafe fn core_boot() -> CoreId {
    unsafe {
        // Safety:
        // we are still setting up all interrupt handlers, locks, etc so
        // we can disable this now until everything is set up.
        cpu::disable_interrupts();
    }

    let core_id: CoreId = CORE_ID_COUNTER.fetch_add(1, Ordering::AcqRel).into();

    // This is critical for the safe access to boot_core_locals
    while BOOT_LOCK.load(Ordering::SeqCst) != core_id.0 {
        spin_loop();
    }

    // Safety: This is only safe as long as we are the only core
    // to access this, which is ensured by the BOOT_LOCK
    let boot_core_locals = unsafe { &mut *addr_of_mut!(BOOT_CORE_LOCALS) };

    cpu::set_gs_base(boot_core_locals as *const CoreStatics as u64);

    // setup locals region for boot of core
    boot_core_locals.core_id = core_id;
    boot_core_locals.self_ptr =
        NonNull::new(boot_core_locals).expect("Core locals should never be at ptr 0");

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
/// This function must only be called once per CPU core, after [`core_boot`] was executed
/// and also after memory and logging was initialized.
///
/// The caller must also ensure that no references or pointers to [super::locals] is held
/// past this function call, as the result of the macro is invalidated and reinitialized by
/// this function
pub unsafe fn init(core_id: CoreId) {
    trace!("init({core_id:?})");
    let apic_id = cpuid::apic_id().into();

    let boot_locals = unsafe {
        // Safety: after `core_boot` and before `init` is done, therefor we have
        // unique access to BOOT_CORE_LOCALS ensured by BOOT_LOCK and function safety gurantees.
        &mut *addr_of_mut!(BOOT_CORE_LOCALS)
    };

    let core_local = unsafe {
        // Safety: during `init` call and `boot_locals` is properly constructed uniuqe reference.
        CoreStatics::initialize(boot_locals, core_id, apic_id)
    };

    unsafe {
        // safety: we are in the kernel boot process and we only access our own
        // core's data
        assert!(CORE_LOCALS_VADDRS[core_id.0 as usize].is_none());
        CORE_LOCALS_VADDRS[core_id.0 as usize] = Some(core_local.self_ptr);
    }

    // set gs base to point to this core_local. That way we can use the locals!
    // macro to access core_local.
    cpu::set_gs_base(core_local.self_ptr.as_ptr() as u64);

    // exit the critical boot section and let next core enter
    BOOT_LOCK.fetch_add(1, Ordering::SeqCst);
    CORE_READY_COUNT.fetch_add(1, Ordering::Release);

    // don't drop core_local, we want it to life forever in static memory.
    // However we can't use a static variable, because we need 1 per core
    core::mem::forget(core_local);
    trace!("Core {}: locals init done", core_id.0);
}

/// Returns this' core [CoreStatics] struct.
///
/// # Safety
///
/// This assumes that `GS` segement was initialized by [init] to point to
/// the [CoreStatics] struct for this core.
pub unsafe fn get_core_statics() -> &'static CoreStatics {
    unsafe {
        let ptr: usize;
        // Safety: we assume that gs_base contains the vaddr of this core's
        // [CoreStatics], which starts with it's own address, therefor we
        // can access the [CoreStatics] vaddr using `gs:[0]`.
        //
        // `gs:[0]` reads the memory at gs_base offset by 0, therefor reading
        // the ptr stored in the first field of [CoreStatics]
        asm! {
            "mov {0}, gs:[0]",
            out(reg) ptr
        }

        &*(ptr as *const CoreStatics)
    }
}
