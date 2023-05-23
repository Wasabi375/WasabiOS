use log::{info, warn};
use shared::sizes::PAGE_SIZE_4K;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use crate::{cpu::gdt::DOUBLE_FAULT_IST_INDEX, debug, framebuffer::Color};
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

macro_rules! interrupt_with_error_fn {
    ($name:ident, $ist_name:ident, $err_name:ident, $block:block) => {
        extern "x86-interrupt" fn $name($ist_name: InterruptStackFrame, $err_name: u64) -> ! $block
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

pub const DOUBLE_FAULT_STACK_SIZE: usize = PAGE_SIZE_4K(1);
interrupt_with_error_fn!(double_fault_handler, stack_frame, err, {
    panic!("DOUBLE_FAULT({err})\n {stack_frame:#?}")
});
