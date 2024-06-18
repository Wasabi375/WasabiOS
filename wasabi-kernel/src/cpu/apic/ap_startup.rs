//! Logic for starting Application Processors(APs)
//!
//! this is called by the Bootstrap Processor(BSP) after initialization
use core::{
    arch::asm,
    hint::spin_loop,
    marker::PhantomData,
    mem,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use shared::{
    lockcell::LockCell,
    rangeset::{Range, RangeSet, RegionRequest},
};
use static_assertions::const_assert;
use thiserror::Error;
use x86_64::{
    structures::paging::{Page, PageSize, PageTableFlags, PhysFrame, Size4KiB},
    PhysAddr, VirtAddr,
};

#[allow(unused)]
use log::{debug, error, info, trace, warn};

use crate::{
    core_local::{get_ready_core_count, get_started_core_count},
    cpu::apic::ipi::{self, Ipi},
    enter_kernel_main, locals, map_page,
    mem::{
        frame_allocator::PhysAllocator,
        page_allocator::PageAllocator,
        page_table::KERNEL_PAGE_TABLE,
        structs::{GuardedPages, Mapped, Unmapped},
        MemError,
    },
    processor_init,
    time::{self, Duration},
    DEFAULT_STACK_PAGE_COUNT, DEFAULT_STACK_SIZE,
};

mod bsp_ctrl_regs {
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use log::trace;
    use x86_64::registers::control::{Cr0, Cr4, Efer};

    use crate::locals;

    static BSP_REGS_STORED: AtomicBool = AtomicBool::new(false);
    static BSP_CR0: AtomicU64 = AtomicU64::new(0);
    static BSP_CR4: AtomicU64 = AtomicU64::new(0);
    static BSP_EFER: AtomicU64 = AtomicU64::new(0);

    /// Store current cores control registers so that they can be applied
    /// to aps.
    ///
    /// This will panic, if called from anywhere but bsp
    pub fn store_bsp_regs() {
        trace!("storing bsp ctrl regs");
        assert!(locals!().is_bsp());
        BSP_CR0.store(Cr0::read_raw(), Ordering::Release);
        BSP_CR4.store(Cr4::read_raw(), Ordering::Release);
        BSP_EFER.store(Efer::read_raw(), Ordering::Release);
        BSP_REGS_STORED.store(true, Ordering::SeqCst);
        trace!("bsp cr0: {:?}", Cr0::read());
        trace!("bsp cr4: {:?}", Cr0::read());
        trace!("bsp efer: {:?}", Efer::read());
    }

    /// Restore control registers from bsp to ap
    ///
    /// # Safety
    /// The caller must guarantee that the bsp regs read with [store_bsp_regs]
    /// are still valid
    pub unsafe fn set_regs() {
        // NOTE logging is not yet initialized in AP when this is called

        // Safety:
        //  we just restore the values used in bsp so this should be safe,
        //  assuming they are not stale.
        unsafe {
            assert!(BSP_REGS_STORED.load(Ordering::SeqCst));
            Cr0::write_raw(BSP_CR0.load(Ordering::Acquire));
            Cr4::write_raw(BSP_CR4.load(Ordering::Acquire));
            Efer::write_raw(BSP_EFER.load(Ordering::Acquire));
        }
    }
}

/// The ptr to the end of the stack for the next ap to boot
///
/// This is 0 when there is no stack available for an ap to boot.
/// Each ap will loop until this is a valid ptr at which point they
/// will use a atomic cmpexchg to take this and replace it with a 0,
/// at which point the bsp will allocate a new stack for the next thread.
///
/// Once AP startup is completed the bsp will check if there is a unused
/// stack left here and free it.
static AP_STACK_PTR: AtomicU64 = AtomicU64::new(0);

static AP_FRAME_ALLOCATED: AtomicBool = AtomicBool::new(false);
static AP_PAGE_ALLOCATED: AtomicBool = AtomicBool::new(false);

const TRAMPOLINE_CODE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/src/cpu/apic/ap_trampoline.bin"));
const_assert!(TRAMPOLINE_CODE.len() as u64 <= Size4KiB::SIZE);

#[allow(dead_code)]
const TEST_COUNTER_OFFSET: u64 = 4;
const PAGE_TABLE_OFFSET: u64 = 3;
const STACK_PTR_OFFSET: u64 = 2;
const ENTRY_POINT_OFFSET: u64 = 1;
const BASE_ADDR_OFFSET: u64 = 0;

/// AP trampoline address
///
/// This must be a valid page and frame start address,
/// that can be accessed in 16bit mode that functions as
/// the start address for all APs.
const TRAMPOLINE_ADDR: u64 = 0x8_000;

/// Reserves the pyhs frame in the lower 0xff000 range of ram
/// for ap startup trampoline
pub fn reserve_phys_frames(allocator: &mut PhysAllocator) {
    if AP_FRAME_ALLOCATED.load(Ordering::Acquire) {
        panic!("ap trampoline frame already allocated");
    }

    let mut boot_range = RangeSet::new();
    boot_range.insert(Range {
        start: TRAMPOLINE_ADDR,
        end: TRAMPOLINE_ADDR + Size4KiB::SIZE,
    });

    let frame = allocator
        .alloc_in_range::<Size4KiB>(RegionRequest::Forced(&boot_range))
        .expect("Failed to allocate multiboot start frame");
    info!("ap startup trampoline frame allocated: {:?}", frame);

    AP_FRAME_ALLOCATED.store(true, Ordering::Release);
}

/// Rserves the page for the identity map of the phys frame used for the
/// ap startup trampoline.
pub fn reserve_pages(allocator: &mut PageAllocator) {
    if AP_PAGE_ALLOCATED.load(Ordering::Acquire) {
        panic!("AP trampoline page already allcoated");
    }

    let vaddr = VirtAddr::new(TRAMPOLINE_ADDR);
    let page = Page::<Size4KiB>::from_start_address(vaddr)
        .expect("TRAMPOLINE_ADDR must be valid page start");
    match allocator.try_allocate_page(page) {
        Ok(_) => debug!("ap startup trampoline page reserved at {page:?}"),
        Err(err) => panic!(
            "failed to identity map AP_STARTUP_VECTOR({:p}): {}",
            vaddr, err
        ),
    }

    AP_PAGE_ALLOCATED.store(true, Ordering::Release);
}

/// Start all application processors.
///
/// After this function exits, the kernel is executed on all cores
/// in the machine
// TODO should this be unsafe. Thechincally this could lead to UB if we start this
// from within an interrupt while this is already running, but not done.
pub fn ap_startup() {
    info!("Starting APs");
    debug!("ap trampoline size: {}", TRAMPOLINE_CODE.len());

    bsp_ctrl_regs::store_bsp_regs();

    // NOTE: dropping this will free the trampoline bytecode, so don't drop
    // this until the end of ap_startup
    let sipi = SipiPayload::new();

    assert_eq!(1, get_ready_core_count(Ordering::SeqCst));

    let mut stack = ApStack::alloc().expect("failed to alloc ap stack");

    {
        trace!("Sending INIT SIPI SIPI: {:#x}", sipi.payload());
        let mut apic = locals!().apic.lock();
        apic.send_ipi(Ipi {
            mode: ipi::DeliveryMode::INIT,
            destination: ipi::Destination::AllExcludingSelf,
        });
        time::sleep_tsc(Duration::new_millis(10));
        let sipi_ipi = Ipi {
            mode: ipi::DeliveryMode::SIPI(sipi.payload()),
            destination: ipi::Destination::AllExcludingSelf,
        };
        apic.send_ipi(sipi_ipi);
        time::sleep_tsc(Duration::new_millis(100));
        apic.send_ipi(sipi_ipi);
    }

    let mut timer = time::read_tsc();
    let mut known_started = 1;
    loop {
        let started = get_started_core_count(Ordering::SeqCst);
        if started == known_started + 1 {
            known_started = started;

            stack = ApStack::alloc().expect("failed to alloc ap stack");

            timer = time::read_tsc();
            continue;
        } else {
            if time::time_since_tsc(timer) > WAIT_FOR_START_TIMEOUT {
                // assume all cores are started
                break;
            } else {
                spin_loop()
            }
        }
    }
    // there should be exactly 1 more stack than aps, so
    // we can free that stack now. Any ap that did not enter startup
    // by now will be stuck waiting for a stack. Therefor we can just
    // assume they don't exits
    trace!("dropping final unused ap stack");
    drop(stack);

    timer = time::read_tsc();
    loop {
        let ready_count = get_ready_core_count(Ordering::SeqCst);
        if ready_count < known_started {
            if time::time_since_tsc(timer) > WAIT_FOR_READY_TIMEOUT {
                panic!("Only {ready_count} cores are ready after timeout, but {known_started} cores weres started");
            } else {
                spin_loop()
            }
        } else if ready_count == known_started {
            info!("all {ready_count} cores ready");
            break;
        } else {
            panic!("somehow {ready_count} cores are ready, but only {known_started} were started");
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ApStack(Mapped<GuardedPages<Size4KiB>>);

/// enum for all ap stack related errors
#[derive(Error, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum ApStackError {
    #[error("failed to alloc ap stack: {0}")]
    Alloc(MemError),
    #[error("A stack already exists in AP_STACK_PTR")]
    StackAlreadyExists,
}

impl From<MemError> for ApStackError {
    fn from(value: MemError) -> Self {
        ApStackError::Alloc(value)
    }
}

impl ApStack {
    const STACK_ALIGN: u64 = 16;

    fn alloc() -> Result<Self, ApStackError> {
        trace!("Allocate ap stack");
        let pages: Unmapped<_> = PageAllocator::get_kernel_allocator()
            .lock()
            .allocate_guarded_pages(DEFAULT_STACK_PAGE_COUNT, true, true)
            .map_err(ApStackError::from)?
            .into();
        let pages = pages.alloc_and_map().map_err(ApStackError::from)?;

        assert_eq!(pages.0.size(), DEFAULT_STACK_SIZE);

        let start = pages.0.start_addr();
        let end = pages.0.end_addr().align_down(Self::STACK_ALIGN);

        assert!(
            start.is_aligned(Self::STACK_ALIGN),
            "allcoated stack start({start:p}) isn't properly aligned?!?"
        );
        assert!(
            end.is_aligned(Self::STACK_ALIGN),
            "allcoated stack end({end:p}) isn't properly aligned?!?"
        );

        match AP_STACK_PTR.compare_exchange(0, end.as_u64(), Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => {
                debug!(
                    "ap stack allocated to {:p}..{:p} (size: {})",
                    start,
                    end,
                    pages.0.size()
                );
                Ok(Self(pages))
            }
            Err(exitsing) => {
                error!(
                    "There is an existing ap stack pointing to {:#016x}, can't create new one.",
                    exitsing
                );
                Self::free(pages);
                Err(ApStackError::StackAlreadyExists)
            }
        }
    }

    /// frees the pages of the AP stack.
    fn free(pages: Mapped<GuardedPages<Size4KiB>>) {
        let unmapped = unsafe {
            // Safety:
            //  we never gave await the address to this memory and we
            //  can't use it, so it's safe to free
            trace!("unmap and free");
            pages.unmap_and_free()
        };
        match unmapped {
            Ok(unmapped) => {
                trace!("free unused stack pages");
                PageAllocator::get_kernel_allocator()
                    .lock()
                    .free_guarded_pages(unmapped.0);
            }
            Err(err) => error!("Failed to unmap unsued ap stack. Leaking memory: {err:?}"),
        }
    }

    fn end(&self) -> VirtAddr {
        self.0 .0.end_addr() + 1
    }
}

impl Drop for ApStack {
    fn drop(&mut self) {
        let end = self.end();
        match AP_STACK_PTR.compare_exchange(end.as_u64(), 0, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => {
                debug!("Freeing unused ap stack");
                ApStack::free(self.0);
            }
            Err(_) => {
                trace!("Ap stack was taken by core, so do nothing on drop");
            }
        }
    }
}

#[allow(unused)]
unsafe extern "C" fn ap_entry() -> ! {
    unsafe {
        // Safety: this is safe during ap_startup
        bsp_ctrl_regs::set_regs();

        // Saftey: only called once for each ap, right here
        processor_init();

        info!("Core {} online", locals!().core_id);

        // Safety: core is initialized
        enter_kernel_main();
    }
}

const WAIT_FOR_START_TIMEOUT: Duration = Duration::new_millis(2000);
const WAIT_FOR_READY_TIMEOUT: Duration = Duration::new_millis(5000);

trait SipiPayloadState {}

enum Construction {}
impl SipiPayloadState for Construction {}

enum Ready {}
impl SipiPayloadState for Ready {}

/// Payload for SIPI (startup inter processor interrupt)
///
/// This holds a single page worth of memory at [Self::phys] and [Self::virt],
/// which is allocated in [Self::new] and "freed" during [Drop].
struct SipiPayload<S: SipiPayloadState = Ready> {
    /// phys addr of the startup vector
    phys: PhysAddr,
    /// virt addr of the startup vector
    virt: VirtAddr,
    /// the phys frame containing the startup vector
    frame: PhysFrame,
    /// the memory page mapping for the startup vector
    page: Page<Size4KiB>,

    _state: PhantomData<S>,
}

impl SipiPayload<Ready> {
    /// constructs a new sipi payload.
    fn new() -> SipiPayload<Ready> {
        if !AP_FRAME_ALLOCATED.load(Ordering::Acquire) && !AP_PAGE_ALLOCATED.load(Ordering::Acquire)
        {
            panic!(
                "ap trampoline frame and page need to be allocated before ap_startup can be called"
            );
        }

        let phys = PhysAddr::new(TRAMPOLINE_ADDR);

        let frame = PhysFrame::from_start_address(phys)
            .expect("Expected valid phys addr in AP_STARTUP_VECTOR");

        // trampoline will be identity mapped. We reserved this page earlier
        let virt = VirtAddr::new(phys.as_u64());
        let page = Page::<Size4KiB>::from_start_address(virt)
            .expect("identity mapping for AP_START_VECTOR must be valid");

        unsafe {
            // Safety:
            //  both frame and page were reserved for this
            map_page!(
                page,
                Size4KiB,
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
                frame
            )
        }
        .expect("failed to identity map ap trampoline");

        trace!("Identity mapped ap trampoline {:p}", phys);

        let mut sipi: SipiPayload<Construction> = SipiPayload {
            phys,
            virt: page.start_address(),
            frame,
            page,
            _state: Default::default(),
        };

        sipi.load_code();

        sipi.set_page_table();
        sipi.set_entry_point();
        sipi.set_base_addr();
        sipi.set_stack_end_ptr();

        let result = SipiPayload {
            phys: sipi.phys,
            virt: sipi.virt,
            frame: sipi.frame,
            page: sipi.page,
            _state: PhantomData,
        };
        // ensure that we don't free the page when converting SipiState
        mem::forget(sipi);
        result
    }

    fn payload(&self) -> u8 {
        let mask: u64 = 0xff000;
        let phys = self.phys.as_u64();
        // assert only mask is part of phys
        assert_eq!(phys & !mask, 0);

        (phys >> 12).try_into().expect(
            "SIPI payload did not fit in u8. 
                    Somethings wrong with our checks.
                    This should have been caught earlier",
        )
    }
}

impl SipiPayload<Construction> {
    fn load_code(&mut self) {
        let dst: *mut u8 = self.virt.as_mut_ptr();
        // Saftey:
        //  * `src` must be [valid] for reads of `count * size_of::<T>()` bytes.
        //      count is src.len()
        //  * `dst` must be [valid] for writes of `count * size_of::<T>()` bytes.
        //      dst is exactly 4KiB and we assert that BOOTSTRAT_CODE is less than 4KiB
        //  * Both `src` and `dst` must be properly aligned.
        //      src is statically allocated by rust and dst is page aligned
        //  * The region of memory beginning at `src` with a size
        //          of `count * size_of::<T>()` bytes must *not* overlap with
        //          the region of memory beginning at `dst` with the same size.
        //      src and dst can't overlap because we allocate dst in a unused
        //      phys frame
        unsafe {
            core::ptr::copy_nonoverlapping(TRAMPOLINE_CODE.as_ptr(), dst, TRAMPOLINE_CODE.len())
        }
    }

    fn set_page_table(&mut self) {
        let page_table: u64;
        unsafe {
            // Safety: reading cr3 is safe in kernel
            asm! {
                "mov {}, cr3",
                out(reg) page_table
            }
        }
        debug!("store page table({:#x}) in sipi payload", page_table);
        self.set_value(PAGE_TABLE_OFFSET, page_table);
    }

    fn set_entry_point(&mut self) {
        debug!(
            "store entry point({:#x}) in sipi payload",
            ap_entry as *const () as u64
        );
        self.set_value(ENTRY_POINT_OFFSET, ap_entry as *const () as u64);
    }

    fn set_base_addr(&mut self) {
        debug!("store base addr({:p}) in sipi payload", self.virt);
        self.set_value(BASE_ADDR_OFFSET, self.virt.as_u64());
    }

    fn set_stack_end_ptr(&mut self) {
        debug!(
            "store stack end ptr({:p}) in sipi payload!",
            AP_STACK_PTR.as_ptr()
        );
        self.set_value(STACK_PTR_OFFSET, AP_STACK_PTR.as_ptr() as u64);
    }

    /// sets a value in the payload at offset relative to the end of the payload
    ///
    /// Offset is given in number of u64 from the end of the payload going backwards so
    /// an offset of 0 would write the last u64 of the payload and a payload of 1
    /// would write the u64 before that, etc
    fn set_value(&mut self, offset: u64, value: u64) {
        use core::mem::{align_of, size_of};
        const WORD_SIZE: u64 = size_of::<u64>() as u64;

        let end = self.virt + TRAMPOLINE_CODE.len() as u64 - WORD_SIZE;
        let target = end - offset * WORD_SIZE;
        trace!("write value({:#x}) to trampoline at {:p}", value, target);
        unsafe {
            let ptr: *mut u64 = target.as_mut_ptr();
            assert!(self.page.start_address() <= target && target <= end);
            assert!(ptr.is_aligned_to(align_of::<u64>()));
            // Safety:
            //   * target is a existing memory address we have mutable access for
            //   * target is aligned to u64
            ptr.write(value);
        }
    }
}

impl<S: SipiPayloadState> SipiPayload<S> {
    /// reads a value in the payload at offset relative to the end of the payload
    ///
    /// Offset is given in number of u64 from the end of the payload going backwards so
    /// an offset of 0 would read the last u64 of the payload and a payload of 1
    /// would read the u64 before that, etc
    #[allow(dead_code)]
    fn read_value(&self, offset: u64) -> u64 {
        use core::mem::{align_of, size_of};
        const WORD_SIZE: u64 = size_of::<u64>() as u64;

        let end = self.virt + TRAMPOLINE_CODE.len() as u64 - WORD_SIZE;
        let target = end - offset * WORD_SIZE;
        unsafe {
            let ptr: *const u64 = target.as_ptr();
            assert!(self.page.start_address() <= target && target <= end);
            assert!(ptr.is_aligned_to(align_of::<u64>()));
            // Safety:
            //   * target is a existing memory address we have shared access for
            //   * target is aligned to u64
            ptr.read()
        }
    }
}

use x86_64::structures::paging::mapper::Mapper;

impl<S: SipiPayloadState> Drop for SipiPayload<S> {
    fn drop(&mut self) {
        // FIXME: freeing this can lead to tripple fault if AP starts after
        //  the drop. Do I care? I mean they should all have started at this time.
        let mut table = KERNEL_PAGE_TABLE.lock();
        let (_frame, _flags, flush) = table
            .unmap(self.page)
            .expect("Failed to unmap ap trampoline during drop");
        flush.flush();
        PageAllocator::get_kernel_allocator()
            .lock()
            .free_page(self.page);
        debug!("ap trampoline unmapped and freed");
        AP_FRAME_ALLOCATED.store(false, Ordering::Release);
        AP_PAGE_ALLOCATED.store(false, Ordering::Release);
    }
}
