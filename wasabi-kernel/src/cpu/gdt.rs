//! utilities to setup the Generald Descriptor Table

use lazy_static::lazy_static;
use log::{debug, info, trace};
use x86_64::{
    registers::segmentation::SS,
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        tss::TaskStateSegment,
    },
    VirtAddr,
};

use crate::cpu::interrupts::DOUBLE_FAULT_STACK_SIZE;

/// IST INDEX used to access separate stack for double fault exceptions
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// set up gdt
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS};
    use x86_64::instructions::tables::load_tss;

    info!("Load GDT");
    GDT.0.load();

    debug!("Load TSS and set CS");
    unsafe {
        // safety: `code` contains a valid cs segment
        CS::set_reg(GDT.1.code);
        // safety: `tss` points to a valid TSS entry
        load_tss(GDT.1.tss);

        // Intel X86 Spec: 3.7.4.1
        // The processor treats the segment base of CS, DS, ES, SS as zero,
        // creating a linear address that is equal to the effective address.
        //
        // Therefor we should not need to set this, because CPU should treat it as 0
        // but QEMU does not appear to care.
        trace!("Load SS");
        SS::set_reg(SegmentSelector::NULL);
    }
}

lazy_static! {
    /// TSS holds address for kernel stack on interrupt
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            /// double fault stack
            static mut STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

            // safety: lazy_static initializer is guared by a spin lock
            let start = VirtAddr::from_ptr(unsafe { &STACK });
            let end = start + DOUBLE_FAULT_STACK_SIZE;
            end
        };
        tss
    };
}

lazy_static! {
    /// the GDT used by this kernel
    static ref GDT: (GlobalDescriptorTable, Segments) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code = gdt.add_entry(Descriptor::kernel_code_segment());
        let tss = gdt.add_entry(Descriptor::tss_segment(&TSS));
        (gdt, Segments { code, tss })
    };
}

/// wrappwer around the segements used by this kernel
struct Segments {
    /// kernel code segment
    code: SegmentSelector,
    /// tss segment
    tss: SegmentSelector,
}
