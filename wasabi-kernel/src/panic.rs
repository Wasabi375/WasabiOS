//! panic handler implementation

use core::{
    panic::PanicInfo,
    ptr::null_mut,
    sync::atomic::{AtomicPtr, Ordering},
};

use interrupt_fn_builder::exception_fn;
use log::error;
use shared::{
    sync::{lockcell::LockCell, Barrier, BarrierTryFailure},
    types::{CoreId, Duration},
};

use crate::{
    core_local::get_ready_core_count,
    cpu::{
        self,
        apic::ipi::{DeliveryMode, Destination, Ipi},
        halt,
    },
    locals,
    logger::LOGGER,
    serial_println,
    time::{sleep_tsc, ticks_in},
};

struct PanicNMIPayload {
    barrier: Barrier,
    panicing: CoreId,
}

static NMI_PAYLOAD: AtomicPtr<PanicNMIPayload> = AtomicPtr::new(null_mut());

// The handler for non maskable interrupts.
//
// This is used to shut down other cores during a panic
exception_fn!(pub nmi_panic_handler, _stack_frame, {
    let payload = unsafe {
        // Saftey: payload must be statically valid once panic is invoked
        &*NMI_PAYLOAD.load(Ordering::Acquire)
    };
    handle_other_core_panic(payload);
});

fn handle_other_core_panic(payload: &PanicNMIPayload) {
    if locals!().core_id == payload.panicing {
        return;
    }

    unsafe {
        // its save to disable interrupts, as we are shutting down
        // we shouldn't get interrupts anyways as this is called from an nmi
        locals!().disable_interrupts();
    }

    payload.barrier.enter();
    halt()
}

/// Disables all other cores and interrupts.
///
/// # Safety:
///
/// This should only be called from panics.
/// The caller must guarantee that `payload` has a static lifetime relative to
/// the panic
unsafe fn panic_disable_cores(payload: &mut PanicNMIPayload) -> Option<BarrierTryFailure> {
    unsafe {
        // Safety: we are in a panic state, so anything relying on interrupts is
        // done for anyways
        locals!().disable_interrupts();
    }

    if let Err(payload) =
        NMI_PAYLOAD.compare_exchange(null_mut(), payload, Ordering::SeqCst, Ordering::SeqCst)
    {
        // some other core paniced before us.
        // wait for panic nmi from other core
        sleep_tsc(Duration::Millis(200));
        // assume nmi failed due to our panic
        // lets just manually jump to "nmi panic handler"
        let payload = unsafe { &*payload };
        handle_other_core_panic(payload);
    }

    let nmi = Ipi {
        mode: DeliveryMode::NMI,
        destination: Destination::AllExcludingSelf,
    };
    // FIXME: deadlock if apic lock is held
    locals!().apic.lock().send_ipi(nmi);

    // proceed even if waiting for other cores times out.
    match payload
        .barrier
        .enter_with_timeout(ticks_in(Duration::Seconds(1)))
    {
        Ok(_) => None,
        Err((_, failure)) => Some(failure),
    }
}

/// Ensures logger works during panic
///
/// This is done by recreating all resources that loggers depend on
/// or if that is not possible, disabling that logger
///
/// # Safety:
///
/// This should onl be called from panics, after all multicore and interrupts are
/// disabled and the framebuffer is useable
unsafe fn recreate_logger() {
    // FIXME recreate loggers in panic
    //      without this logging during panic can deadlock
}

/// Ensures frambuffer is accessible during panic
///
/// # Safety:
///
/// This should onl be called from panics, after all multicore and interrupts are
/// disabled. This function dose not require logging.
unsafe fn recreate_framebuffer() {
    // FIXME: implement
}

/// This function is called on panic.
#[panic_handler]
#[cfg_attr(feature = "test", allow(unreachable_code, unused_variables))]
fn panic(info: &PanicInfo) -> ! {
    let mut payload = PanicNMIPayload {
        barrier: Barrier::new(get_ready_core_count(Ordering::Acquire).into()),
        panicing: locals!().core_id,
    };

    // Safety: we are in a panic handler
    let other_core_timeout = unsafe {
        let timeout = panic_disable_cores(&mut payload);
        recreate_framebuffer();
        recreate_logger();

        // Safety: cores disabled and framebuffer + logger recreated
        #[cfg(feature = "test")]
        crate::testing::panic::test_panic_handler(info);

        timeout
    };

    // Saftey: [LOGGER] is only writen to during the boot process.
    // Either we are in the boot process, in which case only we have access
    // or we aren't in which case everyone only reads
    if unsafe { &*core::ptr::addr_of!(LOGGER) }.is_none() {
        panic_no_logger(info);
    }

    if let Some(timeout) = other_core_timeout {
        error!(target: "PANIC", "timout while waiting for {} processors to shut down", timeout.waiting_on);
    }

    error!(target: "PANIC", "{}", info);

    cpu::halt();
}

/// panic handler if we haven't initialized logging
fn panic_no_logger(info: &PanicInfo) -> ! {
    serial_println!("PANIC during init: {}", info);

    cpu::halt();
}
