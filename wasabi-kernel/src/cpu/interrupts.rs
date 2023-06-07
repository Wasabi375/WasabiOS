//! kernel utilities/handlers for interrupts

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

/// setup idt
pub fn init() {
    info!("Load IDT");
    IDT.load();

    unsafe {
        debug!("interrupts are enabled starting now");
        // safety: this enables interrupts for the kernel after necessary
        // setup is finished
        locals!().enable_interrupts();
    }
}

use paste::paste;

#[doc(hidden)]
macro_rules! int_fn_builder {
    ($int_type:ident) => {
        paste! {
            /// creates a handler function for an $int_type
            #[allow(unused_macros)]
            macro_rules! [<$int_type _fn>]  {
                ($name:ident, $ist_name:ident, $block:tt) => {
                    extern "x86-interrupt" fn $name($ist_name: InterruptStackFrame) {
                        let _guard = $crate::locals!().[<inc_ $int_type>]();
                        $block
                    }
                };
            }

            /// creats a handler function for an $int_type which will panic
            #[allow(unused_macros)]
            macro_rules! [<panic_ $int_type>]  {
                ($name:ident, $ist_name:ident) => {
                    extern "x86-interrupt" fn $name($ist_name: InterruptStackFrame) {
                        let _guard = $crate::locals!().[<inc_ $int_type>]();
                        panic!("$name \n{st:#?}");
                    }
                };
            }


            /// creates a handler function for an $int_type
            #[allow(unused_macros)]
            macro_rules! [<$int_type _with_error_fn>] {
                ($name:ident, $ist_name:ident, $err_name:ident, $block:tt) => {
                    extern "x86-interrupt" fn $name($ist_name: InterruptStackFrame, $err_name: u64) -> !  {
                        let _guard = $crate::locals!().[<inc_ $int_type>]();
                        $block
                    }
                };
            }

            /// creats a handler function for an $int_type which will panic
            #[allow(unused_macros)]
            macro_rules! [<panic_ $int_type _with_error>] {
                ($name:ident) => {
                    extern "x86-interrupt" fn $name(st: InterruptStackFrame, err: u64) {
                        let _guard = $crate::locals!().[<inc_ $int_type>]();
                        panic!("$name \nerr: {err:?}\n{st:#?}");
                    }
                };
            }

            /// creates a page fault $int_type handler
            #[allow(unused_macros)]
            macro_rules! [<$int_type _page_fault_fn>] {
                ($name:ident, $ist_name:ident, $err_name:ident, $block:tt) => {
                    extern "x86-interrupt" fn $name($ist_name: InterruptStackFrame, $err_name: PageFaultErrorCode) {
                        let _guard = $crate::locals!().[<inc_ $int_type>]();
                        $block
                    }
                };
            }
        }
    };
}

int_fn_builder!(interrupt);
int_fn_builder!(exception);

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
