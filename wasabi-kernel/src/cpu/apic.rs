//! Local Apic implementation

use crate::{
    cpu::cpuid::cpuid,
    locals, map_page,
    mem::{MemError, VirtAddrExt},
};
use bit_field::BitField;
use log::{debug, info, trace};
use shared::lockcell::LockCell;
use x86_64::{
    instructions::port::Port,
    registers::model_specific::Msr,
    structures::paging::{Mapper, PageTableFlags, PhysFrame, Size4KiB},
    PhysAddr, VirtAddr,
};

/// MSR address of the local apic base.
const IA32_APIC_BASE: Msr = Msr::new(0x1b);

/// initializes the apic and stores it in [CoreLocals](crate::core_local::CoreLocals)
pub fn init() -> Result<(), MemError> {
    info!("Init Apic...");

    let mut local_apic = locals!().apic.lock();
    if local_apic.is_some() {
        panic!("Apic should only ever be initialized once per core");
    }

    disable_pic();

    let cpuid_apci_info = cpuid(1, None);

    assert!(
        cpuid_apci_info.edx & 1 << 9 != 0,
        "Chip does not support apic"
    );
    trace!("Apic detected");

    let local_apic_id: u8 = cpuid_apci_info
        .ebx
        .get_bits(24..)
        .try_into()
        .unwrap_or_else(|_| panic!("local apic id does not fit in u8"));
    info!("local apic id: {local_apic_id}");

    let apic = Apic::create_from_msr(IA32_APIC_BASE)?;

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

    *local_apic = Some(apic);

    Ok(())
}

/// A struct representing a local apic.
pub struct Apic {
    /// the base vaddr of the Apic, used to access apic registers
    base: VirtAddr,
}

impl Apic {
    /// create a [Apic] reading it's address from the [Msr]
    fn create_from_msr(apic_base: Msr) -> Result<Self, MemError> {
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
        let phys_frame = PhysFrame::<Size4KiB>::containing_address(phys_base);
        let phys_frame = PhysFrame::<Size4KiB>::from_start_address(phys_base)
            .map_err(|_e| ApicCreationError::InvalidBase(phys_base))?;

        let apic_table_flags: PageTableFlags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_EXECUTE
            | PageTableFlags::NO_CACHE;

        let page = unsafe {
            // Safety: new page with apic frame (only used here) and is therefor safe
            map_page!(Size4KiB, apic_table_flags, phys_frame)
                .map_err(|e| ApicCreationError::Mem(e))?
        };

        let virt_base = page.start_address();
        debug!("Create apic base at addr: Phys {phys_base:p}, Virt {virt_base:p}");

        Ok(Apic { base: virt_base })
    }

    /// calculate [VirtAddr] for the given [Offset]
    fn offset(&self, offset: Offset) -> VirtAddr {
        self.base + offset as u64
    }

    /// returns the [Id] of this [Apic]
    fn id(&self) -> Id {
        // Safety: [offset] returns valid virt addrs assuming the apic base is valid
        let id = unsafe { self.offset(Offset::Id).as_volatile() };
        id.read()
    }

    /// issues a End of interrupt to the apic.
    ///
    /// # Safety
    ///
    /// caller must ensure that we are actually ending an interupt when calling this
    /// and that this is only executed once per interrupt
    pub unsafe fn eoi() {
        unsafe {
            use shared::lockcell::LockCellInternal;

            // Safety: this is only safe, because we execute an atomic operation
            // on the apic register, therefor we can ignore the lock here.
            let apic = locals!().apic.get().as_ref().unwrap();

            // Safety: all 32bit writes to an apic register should be atomic therefor
            // this is safe even with a immutable Apic reference
            apic.offset(Offset::EndOfInterrupt)
                .as_volatile_mut()
                .write(0u32);
        }
    }
}

/// Offset of different registers into an [Apic]
#[allow(missing_docs)]
#[repr(usize)]
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
