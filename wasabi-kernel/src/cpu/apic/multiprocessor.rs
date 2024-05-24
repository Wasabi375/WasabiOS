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
    core_local::{self, get_ready_core_count, get_started_core_count},
    cpu::{
        apic::{
            self,
            ipi::{self, Ipi},
        },
        cpuid, gdt, halt, interrupts,
    },
    locals, map_page,
    mem::{
        frame_allocator::{PhysAllocator, WasabiFrameAllocator},
        page_allocator::PageAllocator,
        structs::{GuardedPages, Mapped, Unmapped},
        MemError,
    },
    time::{self, Duration},
    todo_warn, DEFAULT_STACK_PAGE_COUNT, DEFAULT_STACK_SIZE,
};

/// The start phys/virt addr for the ap startup trampoline.
///
/// 0 represents an empty state.
///
/// # Safety:
///
/// When changing this value the following guarantees must be held:
///     * It must either be set to 0
///     * When setting it to !0 the Page and Frame of size 4KiB must
///         be reserved und unused
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

const BOOTSTRAP_CODE: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/src/cpu/apic/ap_trampoline_minimal.bin"
));
const_assert!(BOOTSTRAP_CODE.len() as u64 <= Size4KiB::SIZE);
const PAGE_TABLE_OFFSET: u64 = 3;
const STACK_PTR_OFFSET: u64 = 2;
const ENTRY_POINT_OFFSET: u64 = 1;
const BASE_ADDR_OFFSET: u64 = 0;

/// Reserves the pyhs frame in the lower 0xff000 range of ram
/// for ap startup trampoline
///
/// # Safety:
/// this must be called during bsp boot and [reserve_pages] must also be called
/// after this finished and before ap startup is executed
pub fn reserve_phys_frames(allocator: &mut PhysAllocator) {
    {
        let mut boot_range = RangeSet::new();
        boot_range.insert(Range {
            start: 0,
            end: Size4KiB::SIZE,
        });

        let frame = allocator
            .alloc_in_range::<Size4KiB>(RegionRequest::Forced(&boot_range))
            .expect("Failed to allocate 0 frame");
        warn!("Zero Frame: {:?}", frame);
    }

    let mut boot_range = RangeSet::new();
    boot_range.insert(Range {
        start: 0x01000,
        end: 0xff000 + Size4KiB::SIZE,
    });

    let frame = allocator
        .alloc_in_range::<Size4KiB>(RegionRequest::Forced(&boot_range))
        .expect("Failed to allocate multiboot start frame");
    AP_STARTUP_VECTOR.store(frame.start_address().as_u64(), Ordering::Release);

    debug!("Multiboot start frame reserved at {frame:?}");
}

/// Rserves the page for the identity map of the phys frame used for the
/// ap startup trampoline.
///
/// # Safety
/// this must be called after [reserve_phys_frames] during bsp boot
/// and before ap startup
pub fn reserve_pages(allocator: &mut PageAllocator) {
    let paddr = AP_STARTUP_VECTOR.load(Ordering::Acquire);
    if paddr == 0 {
        panic!("AP_STARTUP_VECTOR is empty. Can't reserve virt page for identity mapping");
    }
    // we want an identity map so this is save
    let vaddr = VirtAddr::new(paddr);
    let page = Page::<Size4KiB>::from_start_address(vaddr)
        .expect("identity mapping for AP_START_VECTOR must be valid");
    match allocator.try_allocate_page(page) {
        Ok(_) => debug!("Mutliboot start page reserved at {page:?}"),
        // TODO I could retry with a new startup vector?
        Err(err) => panic!(
            "failed to identity map AP_STARTUP_VECTOR({:p}): {}",
            vaddr, err
        ),
    }
}

/// Start all application processors.
///
/// After this function exits, the kernel is executed on all cores
/// in the machine
// TODO temp
#[allow(unreachable_code, unused_mut, unused_variables)]
pub fn ap_startup() {
    info!("Starting APs");
    debug!("ap trampoline size: {}", BOOTSTRAP_CODE.len());

    bsp_ctrl_regs::store_bsp_regs();

    // NOTE: dropping this will free the trampoline bytecode, so don't drop
    // this until the end of ap_startup
    let mut sipi = SipiPayload::new();

    if log::log_enabled!(log::Level::Debug) {
        info!("Sipi Trampoline code: ");
        sipi.dump_assembly();
    }

    assert_eq!(1, get_ready_core_count(Ordering::SeqCst));

    let mut stack = ApStack::alloc().expect("failed to alloc ap stack");

    todo_warn!("AP-startup not working");
    return;

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
    // they don't exits
    trace!("dropping final unused ap stack");
    drop(stack);
    info!("test count: {:#X}", sipi.read_value(4));
    sipi.dump_assembly();

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

unsafe extern "C" fn ap_entry() -> ! {
    // TODO unify with kernel_init
    unsafe {
        // Safety: this is safe during ap_startup
        bsp_ctrl_regs::set_regs();
        // Safety: for ap only called in ap_entry
        let core_id = core_local::core_boot();
        // Safety: inherently unsafe and can crash, but if cpuid isn't supported
        // we will crash at some point in the future anyways, so we might as well
        // crash early
        cpuid::check_cpuid_usable();

        // Safety: after core_boot and mem+logging is initialized already by bsp
        core_local::init(core_id);

        gdt::init();
        interrupts::init();

        apic::init().unwrap();

        assert!(locals!().interrupts_enabled());

        info!("Core {} online", core_id);
    }

    // TODO somehow call into kernel main function
    halt();
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

        // trampoline must be identity mapped
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
        sipi.set_real_mode_data_segment();
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

    // thanks to https://github.com/jasoncouture/oxidized/blob/main/kernel/src/arch/arch_x86_64/cpu/mod.rs#L90
    fn dump_assembly(&self) {
        use alloc::{format, string::String};
        use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, Mnemonic, NasmFormatter};

        let mut buffer = [0u8; 4096];
        const HEXBYTES_COLUMN_BYTE_LENGTH: usize = 10;

        unsafe {
            core::ptr::copy_nonoverlapping(
                self.virt.as_ptr(),
                buffer.as_mut_ptr(),
                BOOTSTRAP_CODE.len(),
            );
        }

        let buffer = &buffer[0..BOOTSTRAP_CODE.len()];
        let mut decoder = Decoder::with_ip(16, buffer, 0, DecoderOptions::NONE);

        // Formatters: Masm*, Nasm*, Gas* (AT&T) and Intel* (XED).
        // For fastest code, see `SpecializedFormatter` which is ~3.3x faster. Use it if formatting
        // speed is more important than being able to re-assemble formatted instructions.
        let mut formatter = NasmFormatter::new();

        // Change some options, there are many more
        formatter.options_mut().set_digit_separator("`");
        formatter.options_mut().set_first_operand_char_index(10);

        // String implements FormatterOutput
        let mut output = String::new();

        let mut instruction = Instruction::default();

        while decoder.can_decode() {
            // There's also a decode() method that returns an instruction but that also
            // means it copies an instruction (40 bytes):
            //     instruction = decoder.decode();
            decoder.decode_out(&mut instruction);
            // The first jump we hit in 16 bit mode, is the jump to 64 bit mode.
            // Update the decoder accordingly.
            if instruction.code().mnemonic() == Mnemonic::Jmp && decoder.bitness() == 16 {
                decoder = Decoder::with_ip(64, buffer, 0, DecoderOptions::NONE);
                let rip = instruction.ip() as usize + instruction.len();
                decoder.set_position(rip).unwrap();
                decoder.set_ip(rip as u64);
            }

            // Format the instruction ("disassemble" it)
            output.clear();
            formatter.format(&instruction, &mut output);
            let mut final_output: String = String::new();
            final_output.push_str(format!("{:016X}: ", instruction.ip()).as_str());
            let start_index = (instruction.ip()) as usize;
            let instr_bytes = &buffer[start_index..start_index + instruction.len()];
            for b in instr_bytes.iter() {
                final_output.push_str(format!("{:02X}", b).as_str());
            }
            if instr_bytes.len() < HEXBYTES_COLUMN_BYTE_LENGTH {
                for _ in 0..HEXBYTES_COLUMN_BYTE_LENGTH - instr_bytes.len() {
                    final_output.push_str("  ");
                }
            }
            final_output.push_str(format!(" {}", output).as_str());
            debug!("{}", final_output);
            //debug!("{:016X} {}", instruction.ip(), output);
        }
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
        let page_table: u64;
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

    fn set_stack_end_ptr(&mut self) {
        trace!("store stack end ptr in sipi payload!");
        self.set_value(STACK_PTR_OFFSET, AP_STACK_PTR.as_ptr() as u64);
    }

    fn set_real_mode_data_segment(&mut self) {
        const DS_MAGIC: &[u8] = &[0x90, 0x90, 0xB9, 0xCD, 0xAB, 0x90, 0x90];
        let memory: *mut u8 = self.virt.as_mut_ptr();
        let memory = unsafe {
            // Safety:
            //  virt is a valid ptr for 4KiB and BOOTSTRAP_CODE is shorter than 4KiB
            core::slice::from_raw_parts_mut(memory, BOOTSTRAP_CODE.len())
        };
        let ds_magic_index = memory
            .windows(7)
            .enumerate()
            .find(|(_indx, it)| *it == DS_MAGIC)
            .map(|(i, _)| i)
            .expect("Could not find DS_MAGIC in ap trampoline");
        let ds_index = ds_magic_index + 3;

        let ds_value = self.virt.as_u64();
        let ds_value = ds_value >> 4;
        assert!(ds_value & !0xff00 == 0);

        trace!("store real mode ds({:#X})", ds_value);
        memory[ds_index] = (ds_value & 0xff) as u8;
        memory[ds_index + 1] = ((ds_value >> 8) & 0xff) as u8;
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
    fn read_value(&self, offset: u64) -> u64 {
        use core::mem::{align_of, size_of};
        const WORD_SIZE: u64 = size_of::<u64>() as u64;

        let end = self.virt + BOOTSTRAP_CODE.len() as u64 - WORD_SIZE;
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
