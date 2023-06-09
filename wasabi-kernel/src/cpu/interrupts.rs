//! kernel utilities/handlers for interrupts

use interrupt_fn_builder::{exception_fn, exception_page_fault_fn, exception_with_error_fn};
use log::{debug, info, warn};
use shared::sizes::KiB;
use x86_64::{
    registers::control::Cr2,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode},
};

use crate::{
    cpu::{gdt::DOUBLE_FAULT_IST_INDEX, halt},
    locals,
};
use lazy_static::lazy_static;

lazy_static! {
    /// The interrupt descriptor table used by this kernel
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                // safety: [DOUBLE_FAULT_IST_INDEX] is a valid stack index
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt
    };
}

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

exception_fn!(breakpoint_handler, stack_frame, {
    warn!("breakpoint hit at\n{stack_frame:#?}");
});

/// the stack size for the double fault exception stack
///
/// DF uses a separate stack, in case DF was caused by a stack overflow
pub const DOUBLE_FAULT_STACK_SIZE: usize = KiB(4 * 5);

exception_with_error_fn!(double_fault_handler, stack_frame, err, {
    panic!("DOUBLE FAULT({err})\n{stack_frame:#?}")
});

exception_page_fault_fn!(page_fault_handler, stack_frame, page_fault, {
    warn!(
        "PAGE FAULT:\nAccessed Address: {:p}\nError code: {page_fault:?}\n{stack_frame:#?}",
        Cr2::read()
    );
    // TODO
    halt();
});
