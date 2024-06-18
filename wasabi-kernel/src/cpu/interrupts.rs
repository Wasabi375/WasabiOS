//! kernelutilities/handlers for interrupts

use core::{cell::UnsafeCell, fmt};

#[allow(unused_imports)]
use log::{debug, error, info, warn};
use shared_derive::U8Enum;

use super::apic::Apic;
use crate::{
    cpu::gdt::{DOUBLE_FAULT_IST_INDEX, PAGE_FAULT_IST_INDEX},
    locals,
    prelude::ReadWriteCell,
};
use interrupt_fn_builder::exception_fn;
use shared::lockcell::{LockCell, RWLockCell};
use thiserror::Error;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

/// The function type used for interrupt handlers
pub type InterruptFn =
    fn(interrupt_vector: InterruptVector, stack_frame: InterruptStackFrame) -> Result<(), ()>;

/// An interrupt vector is 8bits so there are `256` total interrupts.
/// However the first `32` are for exceptions that are handled in a special
/// way, therefore we only allow `256 - 32 = 224` handlers.
pub const MAX_INTERRUPT_HANDLER_COUNT: usize = 256 - 32;

/// A struct containing all state related to handling interrupts by the kernel
pub struct InterruptHandlerState {
    /// RW locked array holding all interrupt handlers or None.
    ///
    /// The index into the array is `interrupt_vector - 32`. We don't store
    /// handlers for the first 32 interrupts, as those are used as exceptions handlers
    /// by the CPU and have a different function signature.
    handlers: ReadWriteCell<[Option<InterruptFn>; MAX_INTERRUPT_HANDLER_COUNT]>,
    /// The kernel idt used by a processor
    idt: UnsafeCell<InterruptDescriptorTable>,
}

impl InterruptHandlerState {
    /// Creates a new [InterruptHandlerState]
    ///
    /// containing memory for an IDT and the handler function pointers
    pub const fn new() -> Self {
        InterruptHandlerState {
            handlers: ReadWriteCell::new([None; MAX_INTERRUPT_HANDLER_COUNT]),
            idt: UnsafeCell::new(InterruptDescriptorTable::new()),
        }
    }

    fn idt(&self) -> &InterruptDescriptorTable {
        // Saftey: the only time this is accessed as mutable is during `interrupts::init`
        // which does not alias this
        unsafe { &*self.idt.get() }
    }

    /// Initializes IDT
    ///
    /// # Safety:
    ///
    /// `&self` must be fullfill all guarantees of `&mut self`
    unsafe fn init(&self) {
        // Saftey: see [Self::init]
        let idt = unsafe { &mut *self.idt.get() };

        // init_all_default must be called first or otherwise, this will
        // override any interrupts
        default_handlers::init_all_default_interrupt_handlers(idt);
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            // we have to manually set double_fault and page fault in order to set the stack index
            idt.double_fault
                .set_handler_fn(default_handlers::double_fault)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);

            idt.page_fault
                .set_handler_fn(default_handlers::page_fault_handler)
                .set_stack_index(PAGE_FAULT_IST_INDEX);
        }
    }
}

/// setup idt and enable interrupts
///
/// # Safety:
///
/// must be called during processor startup, after logging and core_locals
/// are initialized
pub unsafe fn init() {
    info!("Load IDT");

    let interrupt_state = &locals!().interrupt_state;
    unsafe {
        // we have uniuq access here, becasue we are in a processor startup
        interrupt_state.init();
    }

    interrupt_state.idt().load();

    unsafe {
        debug!("interrupts are enabled starting now");
        // safety: this enables interrupts for the kernel after necessary
        // setup is finished
        locals!().enable_interrupts();

        assert!(locals!().interrupts_enabled());
    }
}

/// The different interrupt vectors used by the kernel
#[derive(U8Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[allow(missing_docs)]
pub enum InterruptVector {
    Timer = 55,
    Test = 255,
}

impl fmt::Display for InterruptVector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:?}({})", self, *self as u8))
    }
}

#[derive(Error, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum InterruptRegistrationError {
    #[error("Interrupt vector({0}) was already registered")]
    InterruptVectorInUse(InterruptVector),
    #[error("Interrupt vector({0}) was never registered")]
    InterruptVectorNotRegistered(InterruptVector),
    #[error("No Interrupt vector registered")]
    NoRegisteredVector,
}

/// registers a new `handler` for the interrupt `vector`.
///
/// Fails with [InterruptRegistrationError::InterruptVectorInUse] if a handler is already
/// registered for this `vector`
pub fn register_interrupt_handler(
    vector: InterruptVector,
    handler: InterruptFn,
) -> Result<(), InterruptRegistrationError> {
    check_interrupt_vector(vector);
    let mut handler_guard = locals!().interrupt_state.handlers.lock();
    let handlers = &mut handler_guard;

    let index = (vector as u8 - 32) as usize;

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
pub fn unregister_interrupt_handler(
    vector: InterruptVector,
) -> Result<InterruptFn, InterruptRegistrationError> {
    check_interrupt_vector(vector);
    let mut handler_guard = locals!().interrupt_state.handlers.lock();
    let handlers = &mut handler_guard;

    let index = (vector as u8 - 32) as usize;

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
pub fn get_interrupt_handler(vector: InterruptVector) -> Option<InterruptFn> {
    check_interrupt_vector(vector);

    let vector = vector as u8;
    let index = (vector - 32) as usize;
    locals!().interrupt_state.handlers.read()[index]
}

/// asserts that the `vector` is a valid interrupt handler.
///
/// ```
/// # let vector = 35;
/// assert!(vector >= 32 && vector <= 255);
/// ```
fn check_interrupt_vector(vector: InterruptVector) {
    // TODO InterruptVector is enum so I should be able to check all
    //  variances at compile time
    assert!(
        vector as u8 >= 32,
        "interrupt handler called with invalid vector of {vector}"
    );
}

exception_fn!(breakpoint_handler, stack_frame, {
    warn!("breakpoint hit at\n{stack_frame:#?}");
});

/// the number of pages for the double fault exception stack
///
/// DF uses a separate stack, in case DF was caused by a stack overflow
pub const DOUBLE_FAULT_STACK_PAGE_COUNT: u64 = 2;

/// the number of pages for the page fault exception stack
///
/// PF uses a separate stack, in case PF was caused by a stack overflow
pub const PAGE_FAULT_STACK_PAGE_COUNT: u64 = 2;

/// generic interrupt handler, that is called for any interrupt handler with
/// `interrupt_vector >= 32`.
fn interrupt_handler(interrupt_vector: u8, int_stack_frame: InterruptStackFrame) {
    let interrupt_vector = match InterruptVector::try_from(interrupt_vector) {
        Ok(v) => v,
        Err(_) => panic!("Interrupt {interrupt_vector} not defined: \n{int_stack_frame:#?}"),
    };

    let handler = get_interrupt_handler(interrupt_vector);

    if let Some(handler) = handler {
        match handler(interrupt_vector, int_stack_frame) {
            Ok(_) => unsafe {
                // TODO only eoi on fixed delivery mode.
                // if the delivery mode is  NMI, SMI, INIT, ExtINT, the start-up, or INIT-Deassert
                // we shouldn't eoi. Not sure if we ever handle those types of interrupts

                // Safety: we sucessfully handled an interrupt, so we can eoi
                Apic::eoi();
            },
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
