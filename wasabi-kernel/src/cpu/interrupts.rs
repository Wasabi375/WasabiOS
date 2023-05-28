use log::{info, warn};
use shared::sizes::KiB;
use x86_64::{
    registers::control::Cr2,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode},
};

use crate::cpu::{gdt::DOUBLE_FAULT_IST_INDEX, halt};
use lazy_static::lazy_static;

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt
    };
}

pub fn init() {
    info!("Load IDT");
    IDT.load();
}

macro_rules! interrupt_fn {
    ($name:ident, $ist_name:ident, $block:block) => {
        extern "x86-interrupt" fn $name($ist_name: InterruptStackFrame) $block
    };
}

#[allow(unused_macros)]
macro_rules! panic_interrupt {
    ($name:ident) => {
        extern "x86-interrupt" fn $name(st: InterruptStackFrame) {
            panic!("$name \n{st:#?}");
        }
    };
}

macro_rules! interrupt_with_error_fn {
    ($name:ident, $ist_name:ident, $err_name:ident, $block:block) => {
        extern "x86-interrupt" fn $name($ist_name: InterruptStackFrame, $err_name: u64) -> ! $block
    };
}

#[allow(unused_macros)]
macro_rules! panic_interrupt_with_error {
    ($name:ident) => {
        extern "x86-interrupt" fn $name(st: InterruptStackFrame, err: u64) {
            panic!("$name \nerr: {err:?}\n{st:#?}");
        }
    };
}

macro_rules! interrupt_page_fault_fn {
    ($name:ident, $ist_name:ident, $err_name:ident, $block:block) => {
        extern "x86-interrupt" fn $name($ist_name: InterruptStackFrame, $err_name: PageFaultErrorCode) $block
    };
}

interrupt_fn!(breakpoint_handler, stack_frame, {
    warn!("breakpoint hit at\n{stack_frame:#?}");
});

pub const DOUBLE_FAULT_STACK_SIZE: usize = KiB(4 * 5);
interrupt_with_error_fn!(double_fault_handler, stack_frame, err, {
    panic!("DOUBLE FAULT({err})\n{stack_frame:#?}")
});

interrupt_page_fault_fn!(page_fault_handler, stack_frame, page_fault, {
    warn!(
        "PAGE FAULT:\nAccessed Address: {:p}\nError code: {page_fault:?}\n{stack_frame:#?}",
        Cr2::read()
    );
    halt();
});
