use lazy_static::lazy_static;
use x86_64::{
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

    GDT.0.load();

    unsafe {
        CS::set_reg(GDT.1.code);
        load_tss(GDT.1.tss);
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
