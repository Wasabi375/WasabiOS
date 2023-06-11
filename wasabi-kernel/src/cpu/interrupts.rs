//! kernel utilities/handlers for interrupts

#[allow(unused_imports)]
use log::{debug, error, info, warn};

use crate::{cpu::gdt::DOUBLE_FAULT_IST_INDEX, locals, prelude::ReadWriteCell};
use interrupt_fn_builder::exception_fn;
use lazy_static::lazy_static;
use shared::{
    lockcell::{LockCell, RWLockCell},
    sizes::KiB,
};
use thiserror::Error;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

/// The function type used for interrupt handlers
pub type InterruptFn = fn(interrupt_vector: u8, stack_frame: InterruptStackFrame) -> Result<(), ()>;

lazy_static! {
    /// The interrupt descriptor table used by this kernel
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        // init_all_default must be called first or otherwise, this will
        // override any interrupts
        default_handlers::init_all_default_interrupt_handlers(&mut idt);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            // we have to manually set double_fault in order to set the stack index
            idt.double_fault
                .set_handler_fn(default_handlers::double_fault)
                // safety: [DOUBLE_FAULT_IST_INDEX] is a valid stack index
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }
        idt
    };
}

/// RW locked array holding all interrupt handlers or None.
///
/// The index into the array is `interrupt_vector - 32`. We don't store
/// handlers for the first 32 interrupts, as those are used as exceptions handlers
/// by the OS and have a different function signature.
static INTERRUPT_HANDLERS: ReadWriteCell<[Option<InterruptFn>; 256 - 32]> =
    ReadWriteCell::new_preemtable([None; 256 - 32]);

/// setup idt and enable interrupts
pub fn init() {
    info!("Load IDT");
    IDT.load();

    unsafe {
        debug!("interrupts are enabled starting now");
        // safety: this enables interrupts for the kernel after necessary
        // setup is finished
        locals!().enable_interrupts();

        assert!(locals!().interrupts_enabled());
    }
}

#[derive(Error, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum InterruptRegistrationError {
    #[error("Interrupt vector({0}) was already registered")]
    InterruptVectorInUse(u8),
    #[error("Interrupt vector({0}) was never registered")]
    InterruptVectorNotRegistered(u8),
}

/// registers a new `handler` for the interrupt `vector`.
///
/// Fails with [InterruptRegistrationError::InterruptVectorInUse] if a handler is already
/// registered for this `vector`
pub fn register_interrupt_handler(
    vector: u8,
    handler: InterruptFn,
) -> Result<(), InterruptRegistrationError> {
    check_interrupt_vector(vector);
    let mut handler_guard = INTERRUPT_HANDLERS.lock();
    let handlers = &mut handler_guard;

    let index = (vector - 32) as usize;

    if handlers[index].is_some() {
        return Err(InterruptRegistrationError::InterruptVectorInUse(vector));
    }

    handlers[index] = Some(handler);

    Ok(())
}

/// unregisteres the previously registered handler for `vector`.
///
/// Fails with [InterruptRegistrationError::InterruptVectorNotRegistered] if no handler
/// was registered for the handler
pub fn unregister_interrupt_handler(vector: u8) -> Result<InterruptFn, InterruptRegistrationError> {
    check_interrupt_vector(vector);
    let mut handler_guard = INTERRUPT_HANDLERS.lock();
    let handlers = &mut handler_guard;

    let index = (vector - 32) as usize;

    if let Some(handler) = handlers[index] {
        Ok(handler)
    } else {
        Err(InterruptRegistrationError::InterruptVectorNotRegistered(
            vector,
        ))
    }
}

/// returns the registered handler for `vector` or `None` if none was registered
#[inline]
pub fn get_interrupt_handler(vector: u8) -> Option<InterruptFn> {
    check_interrupt_vector(vector);
    let index = (vector - 32) as usize;
    INTERRUPT_HANDLERS.read()[index]
}

/// asserts that the `vector` is a valid interrupt handler.
///
/// ```
/// # let vector = 35;
/// assert!(vector >= 32 && vector <= 255);
/// ```
fn check_interrupt_vector(vector: u8) {
    assert!(
        vector >= 32,
        "interrupt handler called with invalid vector of {vector}"
    );
}

exception_fn!(breakpoint_handler, stack_frame, {
    warn!("breakpoint hit at\n{stack_frame:#?}");
});

/// the stack size for the double fault exception stack
///
/// DF uses a separate stack, in case DF was caused by a stack overflow
pub const DOUBLE_FAULT_STACK_SIZE: usize = KiB(4 * 5);

/// generic interrupt handler, that is called for any interrupt handler with
/// `interrupt_vector >= 32`.
fn interrupt_handler(interrupt_vector: u8, int_stack_frame: InterruptStackFrame) {
    let handler = get_interrupt_handler(interrupt_vector);

    if let Some(handler) = handler {
        match handler(interrupt_vector, int_stack_frame) {
            Ok(_) => {}
            Err(err) => {
                panic!("interrupt handler for {interrupt_vector} failed with {err:?}");
            }
        }
    } else {
        panic!("Interrupt {interrupt_vector} not handled: \n{int_stack_frame:#?}");
    }
}

// docs are hiden, because this module "only" contains autogenerated interrupt handlers
// "auto generated" here means created by macros.
// there is also [default_handler::init_all_default_interrupt_handlers] which is not
// autogenerated, but only sets them as the handlers in the IDT.
#[doc(hidden)]
mod default_handlers;
