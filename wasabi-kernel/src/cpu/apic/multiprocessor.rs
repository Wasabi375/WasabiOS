//! Logic for starting Application Processors(APs)
//!
//! this is called by the Bootstrap Processor(BSP) after initialization
use core::{
    arch::asm,
    hint::spin_loop,
    marker::PhantomData,
    mem,
    sync::atomic::{AtomicU64, Ordering},
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
    cpu::apic::Apic,
    cpu::halt,
    locals, map_frame,
    mem::{
        frame_allocator::{PhysAllocator, WasabiFrameAllocator},
        page_allocator::PageAllocator,
        structs::{GuardedPages, Mapped, Unmapped},
        MemError,
    },
    time::{self, Duration},
    todo_warn, DEFAULT_STACK_PAGE_COUNT, DEFAULT_STACK_SIZE,
};

/// The start phys addr for the ap startup trampoline.
///
/// 0 represents an empty state.
static AP_STARTUP_VECTOR: AtomicU64 = AtomicU64::new(0);

/// The ptr to the end of the stack for the next ap to boot
///
/// This is 0 when there is no stack available for an ap to boot.
/// Each ap will loop until this is a valid ptr at which point they
/// will use a atomic cmpexchg to take this and replace it with a 0.
static AP_STACK_PTR: AtomicU64 = AtomicU64::new(0);

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
    }

    /// Restore control registers from bsp to ap
    ///
    /// # Safety
    /// The caller must guarantee that the bsp regs read with [store_bsp_regs]
    /// are still valid
    pub unsafe fn set_regs() {
        trace!("restoring bsp ctrl regs to ap");
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

const BOOTSTRAP_CODE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/src/cpu/apic/ap_trampoline.bin"));
const_assert!(BOOTSTRAP_CODE.len() as u64 <= Size4KiB::SIZE);
const PAGE_TABLE_OFFSET: u64 = 3;
const STACK_PTR_OFFSET: u64 = 2;
const ENTRY_POINT_OFFSET: u64 = 1;
const BASE_ADDR_OFFSET: u64 = 0;

pub fn reserve_phys_frames(allocator: &mut PhysAllocator) {
    let mut boot_range = RangeSet::new();
    boot_range.insert(Range {
        start: 0x01000,
        end: 0xff000 + Size4KiB::SIZE,
    });

    let frame = allocator
        .alloc_in_range::<Size4KiB>(RegionRequest::Forced(&boot_range))
        .expect("Failed to allocate multiboot start frame");
    AP_STARTUP_VECTOR.store(frame.start_address().as_u64(), Ordering::Release);

    info!("Multiboot start frame: {frame:?}");
}

pub fn ap_startup() {
    info!("Starting APs");
    debug!("ap trampoline size: {}", BOOTSTRAP_CODE.len());

    bsp_ctrl_regs::store_bsp_regs();

    let mut sipi = SipiPayload::new();

    assert_eq!(1, get_ready_core_count(Ordering::SeqCst));

    let mut stack = ApStack::alloc().expect("failed to alloc ap stack");

    todo_warn!("IPI INIT");
    time::sleep_tsc(Duration::new_millis(10));
    todo_warn!("IPI SIPI");
    time::sleep_tsc(Duration::new_millis(100));
    todo_warn!("IPI SIPI");

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
    // they don't exits
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
        let mut pages: Unmapped<_> = PageAllocator::get_kernel_allocator()
            .lock()
            .allocate_guarded_pages(DEFAULT_STACK_PAGE_COUNT, true, true)
            .map_err(ApStackError::from)?
            .into();
        let pages = pages.alloc_and_map().map_err(ApStackError::from)?;

        assert_eq!(pages.0.size(), DEFAULT_STACK_SIZE);

        let start = pages.0.start_addr();
        let end = pages.0.end_addr() + 1;

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

pub unsafe extern "C" fn ap_entry() -> ! {
    // TODO ap final setup
    // TODO somehow call into kernel main function
    halt();
}

const WAIT_FOR_START_TIMEOUT: Duration = Duration::new_millis(200);
const WAIT_FOR_READY_TIMEOUT: Duration = Duration::new_millis(500);

trait SipiPayloadState {}

enum Construction {}
impl SipiPayloadState for Construction {}

enum Ready {}
impl SipiPayloadState for Ready {}

/// Payload for SIPI (startup inter processor interrupt)
///
/// This holds a single page worth of memory at [Self::phys] and [Self::virt],
/// which is allocated in [Self::new] and "freed" during [Drop].
///
/// The phisical frame is released back to [AP_STARTUP_VECTOR] unless it already
/// contains a frame in which case it is freed back to the frame allocator
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
    ///
    /// This panics if [AP_STARTUP_VECTOR] is `0` and will take "ownership" of
    /// that memory, so only 1 instance of SipiPayload can exist unless a new
    /// vector is written to [AP_STARTUP_VECTOR] after a call to [Self::new].
    ///
    /// It will also create a memory mapping for that frame and
    /// will panic if the mapping fails
    fn new() -> SipiPayload<Ready> {
        let phys = AP_STARTUP_VECTOR.load(Ordering::Acquire);
        if phys == 0 {
            panic!("Can't create SipiPayload. AP_STARTUP_VECTOR is 0");
        }
        let phys = match AP_STARTUP_VECTOR.compare_exchange(
            phys,
            0,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(phys) => PhysAddr::new(phys),
            Err(_) => panic!("Can't create SipiPayload. Lost race for AP_STARTUP_VECTOR"),
        };
        let frame = PhysFrame::from_start_address(phys)
            .expect("Expected valid phys addr in AP_STARTUP_VECTOR");

        let page = unsafe {
            // SAFETEY:
            //  We have exclusive access to the phys-frame and use a new page
            map_frame!(
                Size4KiB,
                PageTableFlags::WRITABLE | PageTableFlags::PRESENT,
                frame
            )
        }
        .expect("failed to map ap startup frame to new page");

        trace!(
            "Mapped ap trampoline {:p} -> {:p}",
            phys,
            page.start_address()
        );

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
            core::ptr::copy_nonoverlapping(BOOTSTRAP_CODE.as_ptr(), dst, BOOTSTRAP_CODE.len())
        }
    }

    fn set_page_table(&mut self) {
        let mut page_table: u64 = 0;
        unsafe {
            // Safety: reading cr3 is safe in kernel
            asm! {
                "mov {}, cr3",
                out(reg) page_table
            }
        }
        trace!("store page table in sipi payload");
        self.set_value(PAGE_TABLE_OFFSET, page_table);
    }

    fn set_entry_point(&mut self) {
        trace!("store entry point in sipi payload");
        self.set_value(ENTRY_POINT_OFFSET, ap_entry as *const () as u64);
    }

    fn set_base_addr(&mut self) {
        trace!("store base addr in sipi payload");
        self.set_value(BASE_ADDR_OFFSET, self.virt.as_u64());
    }

    /// sets a value in the payload at offset relative to the end of the payload
    ///
    /// Offset is given in number of u64 from the end of the payload going backwards so
    /// an offset of 0 would write the last u64 of the payload and a payload of 1
    /// would write the u64 before that, etc
    fn set_value(&mut self, offset: u64, value: u64) {
        use core::mem::{align_of, size_of};
        const WORD_SIZE: u64 = size_of::<u64>() as u64;

        let end = self.virt + BOOTSTRAP_CODE.len() as u64 - WORD_SIZE;
        let target = end - offset * WORD_SIZE;
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

impl<S: SipiPayloadState> Drop for SipiPayload<S> {
    fn drop(&mut self) {
        PageAllocator::get_kernel_allocator()
            .lock()
            .free_page(self.page);

        let phys = self.phys.as_u64();
        match AP_STARTUP_VECTOR.compare_exchange(0, phys, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => {
                trace!("released frame back to AP_STARTUP_VECTOR");
            }
            Err(_) => {
                debug!("AP_STARTUP_VECTOR is full. Freeing frame isntaed");
                unsafe {
                    // SAFETY:
                    //  the frame is no longer used, because we just freed the page.
                    //  It is also no longer accessible via PhysAddr because we
                    //  couldn't store it in AP_STARTUP_VECTOR
                    WasabiFrameAllocator::<Size4KiB>::get_for_kernel()
                        .lock()
                        .free(self.frame)
                }
            }
        }
    }
}
