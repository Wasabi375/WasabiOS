//! utilities to setup the Generald Descriptor Table

use core::{cell::UnsafeCell, mem::MaybeUninit};

use log::{debug, info, trace};
use shared::sync::lockcell::LockCell;
use x86_64::{
    instructions::tables::load_tss,
    registers::segmentation::{Segment, CS, SS},
    structures::{
        gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector},
        paging::Size4KiB,
        tss::TaskStateSegment,
    },
};

use crate::{
    cpu::interrupts::{DOUBLE_FAULT_STACK_PAGE_COUNT, PAGE_FAULT_STACK_PAGE_COUNT},
    mem::{
        page_allocator::PageAllocator,
        structs::{GuardedPages, Unmapped},
        MemError,
    },
};

/// IST INDEX used to access separate stack for double fault exceptions
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// IST INDEX used to access sparate stack for page fault exceptions
pub const PAGE_FAULT_IST_INDEX: u16 = 1;

/// wrappwer around the segements used by this kernel
struct Segments {
    /// kernel code segment
    code: SegmentSelector,
    /// tss segment
    tss: SegmentSelector,
}

/// container for data related to GDT
pub struct GDTInfo {
    inner: UnsafeCell<GDTInner>,
}

struct GDTInner {
    /// the gdt itself
    gdt: MaybeUninit<GlobalDescriptorTable>,

    tss: MaybeUninit<TaskStateSegment>,

    segments: MaybeUninit<Segments>,
}

impl GDTInfo {
    /// Creates a new uninitalized GDT
    pub const fn new_uninit() -> GDTInfo {
        GDTInfo {
            inner: UnsafeCell::new(GDTInner {
                gdt: MaybeUninit::uninit(),
                tss: MaybeUninit::uninit(),
                segments: MaybeUninit::uninit(),
            }),
        }
    }

    /// Setup and load GDT
    ///
    /// # Safety:
    ///
    /// must only ever be called once for each processor during startup
    /// requires core_locals and logging and memory to be initialized
    pub unsafe fn init_and_load(&'static self) {
        let inner: &'static mut GDTInner = unsafe {
            // Safety: see outer safety
            &mut *self.inner.get()
        };

        let tss = inner.tss.write(Self::init_tss());

        let (gdt, segments) = Self::init_gdt(tss);

        let gdt = inner.gdt.write(gdt);
        let segments = inner.segments.write(segments);

        info!("Load GDT");
        gdt.load();

        debug!("Load TSS and set CS");
        unsafe {
            // safety: `code` contains a valid cs segment
            CS::set_reg(segments.code);
            // safety: `tss` points to a valid TSS entry
            load_tss(segments.tss);

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

    fn init_gdt(tss: &'static TaskStateSegment) -> (GlobalDescriptorTable, Segments) {
        let mut gdt = GlobalDescriptorTable::new();
        let code = gdt.append(Descriptor::kernel_code_segment());
        let tss = gdt.append(Descriptor::tss_segment(&tss));
        (gdt, Segments { code, tss })
    }

    fn init_tss() -> TaskStateSegment {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            let stack = Self::allocate_stack(DOUBLE_FAULT_STACK_PAGE_COUNT)
                .expect("failed to allocate stack for double faults");

            stack.end_addr().align_down(16u64)
        };
        tss.interrupt_stack_table[PAGE_FAULT_IST_INDEX as usize] = {
            let stack = Self::allocate_stack(PAGE_FAULT_STACK_PAGE_COUNT)
                .expect("failed to allocate stack for page faults");

            stack.end_addr().align_down(16u64)
        };
        tss
    }

    fn allocate_stack(page_count: u64) -> Result<GuardedPages<Size4KiB>, MemError> {
        let pages: Unmapped<_> = PageAllocator::get_kernel_allocator()
            .lock()
            .allocate_guarded_pages(page_count, true, false)?
            .into();
        let stack = pages.alloc_and_map()?.0;

        Ok(stack)
    }
}
