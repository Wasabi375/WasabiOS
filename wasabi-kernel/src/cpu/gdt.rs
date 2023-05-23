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

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS};
    use x86_64::instructions::tables::load_tss;

    info!("Load GDT");
    GDT.0.load();

    debug!("Load TSS and set CS");
    unsafe {
        CS::set_reg(GDT.1.code);
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
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            static mut STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

            let start = VirtAddr::from_ptr(unsafe { &STACK });
            let end = start + DOUBLE_FAULT_STACK_SIZE;
            end
        };
        tss
    };
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Segments) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code = gdt.add_entry(Descriptor::kernel_code_segment());
        let tss = gdt.add_entry(Descriptor::tss_segment(&TSS));
        (gdt, Segments { code, tss })
    };
}

struct Segments {
    code: SegmentSelector,
    tss: SegmentSelector,
}
