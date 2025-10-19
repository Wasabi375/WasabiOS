//! Local Apic implementation

use core::sync::atomic::Ordering;

use crate::{
    cpu::{cpuid::cpuid, interrupts::register_interrupt_handler},
    locals,
    mem::{
        MemError, frame_allocator::FrameAllocator, page_allocator::PageAllocator,
        page_table::PageTable, ptr::UntypedPtr,
    },
    prelude::TicketLock,
};
use bit_field::BitField;
use shared::{sync::lockcell::LockCell, todo_warn};
use thiserror::Error;
use volatile::{
    Volatile,
    access::{ReadOnly, ReadWrite},
};
use x86_64::{
    PhysAddr,
    instructions::port::Port,
    registers::model_specific::Msr,
    structures::{
        idt::InterruptStackFrame,
        paging::{PageTableFlags, PhysFrame, Size4KiB},
    },
};

use self::{
    ipi::{IPI_STATUS_BIT, Ipi},
    timer::{Timer, TimerData},
};

#[allow(unused_imports)]
use log::{debug, info, trace};

use super::interrupts::{InterruptRegistrationError, InterruptVector};

pub mod ap_startup;
pub mod ipi;
pub mod timer;

/// MSR for the apic base.
static IA32_APIC_BASE: TicketLock<Msr> = TicketLock::new(Msr::new(0x1b));

/// initializes the apic and stores it in [CoreLocals](crate::core_local::CoreLocals)
pub fn init() -> Result<(), ApicCreationError> {
    info!("Init Apic...");

    let mut apic_base = IA32_APIC_BASE.lock();
    let mut local_apic = locals!().apic.lock_uninit();

    disable_pic();

    let cpuid_apci_info = cpuid(1, None);

    assert!(
        cpuid_apci_info.edx.get_bit(9),
        "Chip does not support local apic"
    );

    let local_apic_id: u8 = cpuid_apci_info
        .ebx
        .get_bits(24..)
        .try_into()
        .unwrap_or_else(|_| panic!("local apic id does not fit in u8"));
    debug!("local apic id based on cpuid: {local_apic_id}");

    let mut apic = Apic::create_from_msr(apic_base.as_mut())?;

    let id_reg = apic.id();
    assert_eq!(
        id_reg.id(),
        local_apic_id,
        "local apic id from cpuid and apic did not match"
    );
    assert_eq!(
        id_reg.id(),
        locals!().apic_id.0,
        "apic id in locals()! did not match apic provided id"
    );

    apic.timer().calibrate();

    local_apic.write(apic);
    if locals!().is_ap() {
        locals!().nmi_on_panic.store(true, Ordering::Release);
    }
    info!("Apic initialized");

    Ok(())
}

/// A struct representing a local apic.
pub struct Apic {
    /// the base vaddr of the Apic, used to access apic registers
    base: UntypedPtr,

    /// the timer of this APIC
    timer: TimerData,
}

// Safety: We ensure that access to base is done in a save way
unsafe impl Send for Apic {}
unsafe impl Sync for Apic {}

/// The error used for Apic creation
#[derive(Error, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum ApicCreationError {
    #[error("{0}")]
    Mem(#[from] MemError),
    #[error("Invalid Physical Base {0:p}")]
    InvalidBase(PhysAddr),
    #[error("Failed to register interrupt handler")]
    RegisterInterruptHandler(#[from] InterruptRegistrationError),
}

impl Apic {
    /// create a [Apic] reading it's address from the [Msr]
    fn create_from_msr(apic_base: &mut Msr) -> Result<Self, ApicCreationError> {
        // Safety: reading apic base is ok
        let apic_base_data = unsafe { apic_base.read() };
        let apic_enabled = apic_base_data.get_bit(11);
        let apic_bsp = apic_base_data.get_bit(8);

        assert!(apic_enabled, "Apic global enable is set to 0.");
        assert_eq!(
            apic_bsp,
            locals!().is_bsp(),
            "Apic thinks we are in bsp, but locals!() disagrees"
        );

        let base_addr: u32 = apic_base_data
            .get_bits(12..=35)
            .try_into()
            .expect("24 bits shoudl always fit into u32");
        let base_addr = base_addr << 12;

        let phys_base = PhysAddr::new(base_addr as u64);
        let phys_frame = PhysFrame::<Size4KiB>::from_start_address(phys_base)
            .map_err(|_e| ApicCreationError::InvalidBase(phys_base))?;

        let apic_table_flags: PageTableFlags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_EXECUTE
            | PageTableFlags::NO_CACHE;

        let page = PageAllocator::get_for_kernel()
            .lock()
            .allocate_page_4k()
            .map_err(MemError::from)?;
        unsafe {
            // Safety: new page with apic frame (only used here) and is therefor safe
            PageTable::get_for_kernel()
                .lock()
                .map_kernel(
                    page,
                    phys_frame,
                    apic_table_flags,
                    FrameAllocator::get_for_kernel().lock().as_mut(),
                )
                .map_err(MemError::from)?
                .flush();
        };

        let base_ptr = unsafe {
            // Saftey: we just mapped this page
            UntypedPtr::new(page.start_address())
        }
        .expect("Page start_addr should never be 0");
        debug!("Create apic base at addr: Phys {phys_base:p}, Virt {base_ptr:p}");

        let mut apic = Apic {
            base: base_ptr,
            timer: TimerData::default(),
        };

        // stop the timer in case the bootloader used the apic timer
        apic.timer().stop();

        register_interrupt_handler(InterruptVector::Spurious, spurious_int_handler)?;
        apic.offset_mut(Offset::SpuriousInterruptVector)
            .update(|siv| {
                siv.set_bit(8, true);
                siv.set_bits(0..=7, InterruptVector::Spurious as u8 as u32);
            });

        Ok(apic)
    }

    /// calculate [Volatile] for the given [Offset]
    fn offset(&self, offset: Offset) -> Volatile<&u32, ReadOnly> {
        unsafe {
            // saftey: offset is within allocation
            let base_ptr = self.base.offset(offset as isize);
            // safety: we have read access to apic, so we can read it's registers
            base_ptr.as_volatile()
        }
    }

    /// calculate [Volatile] low and high for the given [Offset]
    #[allow(dead_code)]
    fn offset64(&self, offset: Offset) -> (Volatile<&u32, ReadOnly>, Volatile<&u32, ReadOnly>) {
        unsafe {
            // Saftey: offsets within allocaton
            let vaddr_low = self.base.offset(offset as isize);
            let vaddr_high = vaddr_low.offset(0x10);

            // safety: we have read access to apic, so we can read it's registers
            (vaddr_low.as_volatile(), vaddr_high.as_volatile())
        }
    }

    /// calculate mutable [Volatile] for the given [Offset]
    fn offset_mut(&mut self, offset: Offset) -> Volatile<&mut u32, ReadWrite> {
        unsafe {
            // saftey: offset is within allocation
            let vaddr = self.base.offset(offset as isize);
            // safety: we have mut access to apic, so we can read and write it's registers
            vaddr.as_volatile_mut()
        }
    }

    /// calculate mutable [Volatile] low and high for the given [Offset]
    fn offset64_mut(
        &self,
        offset: Offset,
    ) -> (Volatile<&mut u32, ReadWrite>, Volatile<&mut u32, ReadWrite>) {
        unsafe {
            // Saftey: offsets within allocaton
            let vaddr_low = self.base.offset(offset as isize);
            let vaddr_high = vaddr_low.offset(0x10);

            // safety: we have mut access to apic, so we can read and write it's registers
            (vaddr_low.as_volatile_mut(), vaddr_high.as_volatile_mut())
        }
    }

    /// get access to the apic timer
    pub fn timer(&mut self) -> Timer<'_> {
        Timer { apic: self }
    }

    /// returns the [Id] of this [Apic]
    pub fn id(&self) -> Id {
        let id = self.offset(Offset::Id);
        Id(id.read())
    }

    /// send an Inter-Processor interrupt
    pub fn send_ipi(&mut self, ipi: Ipi) {
        let (low, high) = self.offset64_mut(Offset::InterruptCommand);
        ipi.send(low, high)
    }

    /// Read the status of the InterruptCommand register
    pub fn ipi_status(&mut self) -> ipi::DeliveryStatus {
        let (low, _high) = self.offset64_mut(Offset::InterruptCommand);
        low.read().get_bit(IPI_STATUS_BIT).into()
    }

    /// issues a End of interrupt to the apic.
    ///
    /// # Safety
    ///
    /// caller must ensure that we are actually ending an interupt when calling this
    /// and that this is only executed once per interrupt
    pub unsafe fn eoi() {
        unsafe {
            use shared::sync::lockcell::LockCellInternal;

            // Safety: this is only safe, because we execute an atomic operation
            // on the apic register, therefor we can ignore the lock here.
            let apic = LockCellInternal::<Apic>::get_mut(&locals!().apic);

            apic.offset_mut(Offset::EndOfInterrupt).write(0u32);
        }
    }
}

impl Drop for Apic {
    fn drop(&mut self) {
        todo_warn!("Drop for APIC is not implemented");
        // NOTE: I'm not sure what to do here. Is it even possible to recreate the APIC properly
        // with this setup? I guess we could unmap the APIC page?
    }
}

/// Offset of different registers into an [Apic]
#[allow(missing_docs)]
#[repr(isize)]
pub enum Offset {
    Id = 0x20,
    Version = 0x30,
    TaskPriority = 0x80,
    ArbitrationPriority = 0x90,
    ProcessorPriority = 0xa0,
    EndOfInterrupt = 0xb0,
    RemoteRead = 0xc0,
    LocalDestination = 0xd0,
    DestinationFormat = 0xe0,
    SpuriousInterruptVector = 0xf0,
    InService = 0x100,
    TriggerMode = 0x180,
    InterruptRequest = 0x200,
    ErrorStatus = 0x280,
    InterruptCommand = 0x300,
    TimerLocalVectorTableEntry = 0x320,
    ThermalLocalVectorTableEntry = 0x330,
    PerformanceCounterLocalVectorTableEntry = 0x340,
    LocalInterrupt0VectorTableEntry = 0x350,
    LocalInterrupt1VectorTableEntry = 0x360,
    ErrorVectorTableEntry = 0x370,
    TimerInitialCount = 0x380,
    TimerCurrentCount = 0x390,
    TimerDivideConfiguration = 0x3e0,
    // this is part of extended apic topology see Intel Manual 3A chapter 11.12.8
    // ExtendedApicFeature = 0x400,
    // ExtendedApicControl = 0x410,
    // SpecificEndOfInterrupt = 0x420,
    // InterruptEnable = 0x480,
    // ExtendedInterruptLocalVectorTable = 0x500,
}

/// [Apic] id register
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct Id(u32);

impl Id {
    /// reads the id from the register
    pub fn id(&self) -> u8 {
        self.0
            .get_bits(24..)
            .try_into()
            .expect("Apic id should fit into u8")
    }
}

fn spurious_int_handler(_vec: InterruptVector, stack_frame: InterruptStackFrame) -> Result<(), ()> {
    panic!("spurious interrupt: \n{stack_frame:#x?}");
}

/// disables the 8259 pic
fn disable_pic() {
    // safety: copy pasta to disable pic. This is safe, because the
    // port addrs are the valid addrs for the pic
    unsafe {
        // Disable 8259 immediately, thanks kennystrawnmusic

        let mut cmd_8259a = Port::<u8>::new(0x20);
        let mut data_8259a = Port::<u8>::new(0x21);
        let mut cmd_8259b = Port::<u8>::new(0xa0);
        let mut data_8259b = Port::<u8>::new(0xa1);

        let mut spin_port = Port::<u8>::new(0x80);
        let mut spin = || spin_port.write(0);

        cmd_8259a.write(0x11);
        cmd_8259b.write(0x11);
        spin();

        data_8259a.write(0xf8);
        data_8259b.write(0xff);
        spin();

        data_8259a.write(0b100);
        spin();

        data_8259b.write(0b10);
        spin();

        data_8259a.write(0x1);
        data_8259b.write(0x1);
        spin();

        data_8259a.write(u8::MAX);
        data_8259b.write(u8::MAX);
    };
}
