//! panic handler implementation

use core::{
    panic::PanicInfo,
    ptr::{addr_of, null_mut},
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use interrupt_fn_builder::exception_fn;
use logger::RefLogger;
use shared::{
    sync::{
        barrier::Barrier,
        lockcell::{LockCell, LockCellInternal},
    },
    types::{CoreId, Duration},
};
use uart_16550::SerialPort;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::{
    core_local::{get_ready_core_count, CoreInterruptState},
    cpu::{
        self,
        apic::ipi::{DeliveryMode, Destination, Ipi},
        halt,
    },
    locals,
    logger::{set_global_logger, setup_logger_module_rename, MAX_RENAME_MAPPINGS},
    prelude::UnwrapTicketLock,
    serial::{SERIAL1, SERIAL2},
    time::{sleep_tsc, ticks_in},
};

struct PanicNMIPayload {
    barrier: Barrier,
    panicing: CoreId,
}

static NMI_PAYLOAD: AtomicPtr<PanicNMIPayload> = AtomicPtr::new(null_mut());
// TODO make this core local, once I have a better system for core locals
static IN_PANIC: AtomicBool = AtomicBool::new(false);

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
    IN_PANIC.store(true, Ordering::SeqCst);
    halt()
}

#[derive(Debug, PartialEq, Eq)]
enum CoreDisableResult {
    AllDisabled,
    Timeout,
    NoApicOnAp,
}

/// Disables all other cores and interrupts.
///
/// # Safety
///
/// This should only be called from panics.
/// The caller must guarantee that `payload` has a static lifetime relative to
/// the panic
unsafe fn panic_disable_cores(payload: &mut PanicNMIPayload) -> CoreDisableResult {
    unsafe {
        // Safety: we are in a panic state, so anything relying on interrupts is
        // done for anyways
        locals!().disable_interrupts();
    }

    if !locals!().nmi_on_panic.load(Ordering::Acquire) {
        if locals!().is_bsp() {
            // Safety:
            // We have not yet started any other processors
            return CoreDisableResult::AllDisabled;
        } else {
            // we are in an application processor, that does not have access to apic,
            // so we can't shut down other processors. Everything from here on, is just bad
            return CoreDisableResult::NoApicOnAp;
        }
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
    let apic = unsafe {
        // Safety: We have unique access, because everything else is shut down
        &mut *locals!().apic.shatter()
    };
    apic.send_ipi(nmi);

    // proceed even if waiting for other cores times out.
    let result = match payload
        .barrier
        .enter_with_timeout(ticks_in(Duration::Seconds(1)))
    {
        Ok(_) => CoreDisableResult::AllDisabled,
        Err(_) => CoreDisableResult::Timeout,
    };

    IN_PANIC.store(true, Ordering::SeqCst);
    result
}

/// Ensures logger works during panic
///
/// This is done by recreating all resources that loggers depend on
/// or if that is not possible, disabling that logger
///
/// # Safety
///
/// This should onl be called from panics, after all multicore and interrupts are
/// disabled and the framebuffer is useable
unsafe fn recreate_logger() {
    // Safety: we are in panic reset
    unsafe {
        LockCellInternal::<SerialPort>::shatter_permanent(&SERIAL1);
        LockCellInternal::shatter_permanent(&SERIAL2);
    }

    let mut serial_logger = RefLogger::new(&SERIAL1);
    setup_logger_module_rename(&mut serial_logger);
    serial_logger.init();

    // Safety: we are in panic reset
    unsafe {
        SERIAL_LOGGER = Some(serial_logger);
        set_global_logger((&*addr_of!(SERIAL_LOGGER)).as_ref().unwrap_unchecked());
    }
}

/// static serial logger location for panics
static mut SERIAL_LOGGER: Option<
    RefLogger<
        'static,
        SerialPort,
        UnwrapTicketLock<SerialPort>,
        CoreInterruptState,
        0,
        MAX_RENAME_MAPPINGS,
    >,
> = None;

/// Ensures frambuffer is accessible during panic
///
/// # Safety
///
/// This should onl be called from panics, after all multicore and interrupts are
/// disabled. This function dose not require logging.
unsafe fn recreate_framebuffer() {
    // TODO: implement
}

/// This function is called on panic.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    panic_impl(info);
}

#[cfg_attr(feature = "test", allow(unreachable_code, unused_variables))]
fn panic_impl(info: &PanicInfo) -> ! {
    // TODO this is running in an endless loop if there are no aps
    let core_disable_result = if IN_PANIC.load(Ordering::Acquire) {
        error!(target: "PANIC", "panic in panic! {info:?}");
        CoreDisableResult::AllDisabled
    } else {
        // if we panic in panic we dont send another nmi
        let mut payload = PanicNMIPayload {
            barrier: Barrier::new(get_ready_core_count(Ordering::Acquire).into()),
            panicing: locals!().core_id,
        };

        // Safety: we are in a panic handler
        unsafe {
            let core_disable_result = panic_disable_cores(&mut payload);
            recreate_framebuffer();
            recreate_logger();

            // Safety: cores disabled and framebuffer + logger recreated
            #[cfg(feature = "test")]
            crate::testing::panic::test_panic_handler(info);

            core_disable_result
        }
    };

    match core_disable_result {
        CoreDisableResult::Timeout => {
            error!(target: "PANIC", "timout while waiting for other processors to shut down");
        }
        CoreDisableResult::NoApicOnAp => {
            error!(target: "PANIC", "failed to disable other cores, because we don't have access to apic and nmis");
        }
        _ => {}
    }

    error!(target: "PANIC", "{}", info);

    debug!(target: "PANIC", "cpu halt");

    cpu::halt();
}
