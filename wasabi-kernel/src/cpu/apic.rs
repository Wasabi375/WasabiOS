use core::ops::DerefMut;

use crate::{
    cpu::cpuid::cpuid,
    locals,
    mem::{
        frame_allocator::WasabiFrameAllocator, page_allocator::PageAllocator, KernelPageTable,
        MemError, VirtAddrExt,
    },
    todo_warn,
};
use bit_field::BitField;
use log::{debug, info, trace};
use shared::lockcell::LockCell;
use volatile::{access::ReadOnly, Volatile};
use x86_64::{
    instructions::port::Port,
    registers::model_specific::Msr,
    structures::paging::{Mapper, PageTableFlags, PhysFrame, Size4KiB},
    PhysAddr, VirtAddr,
};

struct APIC {}

const IA32_APIC_BASE: Msr = Msr::new(0x1b);

pub fn init() -> Result<(), MemError> {
    info!("Init Apic...");

    disable_8259();

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

    let apic_base = ApicBase::create_from_msr(IA32_APIC_BASE)?;
    let id_reg = apic_base.id();
    assert_eq!(
        id_reg.read().id(),
        local_apic_id,
        "local apic id from cpuid and apic did not match"
    );
    assert_eq!(
        id_reg.read().id(),
        locals!().apic_id.0,
        "apic id in locals()! did not match apic provided id"
    );

    todo_warn!();
    Ok(())
}

pub struct ApicBase(VirtAddr);

impl ApicBase {
    #[allow(unused_variables, unreachable_code)] // TODO temp
    fn create_from_msr(apic_base: Msr) -> Result<Self, MemError> {
        // Safety: reading apic base is ok
        let apic_base_data = unsafe { apic_base.read() };
        let apic_enabled = apic_base_data.get_bit(11);

        assert!(apic_enabled, "Apic global enable is set to 0.");

        let base_addr: u32 = apic_base_data
            .get_bits(12..=35)
            .try_into()
            .expect("24 bits shoudl always fit into u32");
        let base_addr = base_addr << 12;

        let phys_base = PhysAddr::new(base_addr as u64);
        let phys_frame = PhysFrame::<Size4KiB>::containing_address(phys_base);

        assert_eq!(phys_base, phys_frame.start_address());

        let page = PageAllocator::get_kernel_allocator()
            .lock()
            .allocate_page_4k()?;

        unsafe {
            let apic_table_flags: PageTableFlags = PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::NO_EXECUTE
                | PageTableFlags::NO_CACHE;
            KernelPageTable::get()
                .lock()
                .map_to(
                    page,
                    phys_frame,
                    apic_table_flags,
                    WasabiFrameAllocator::<Size4KiB>::get_for_kernel()
                        .lock()
                        .deref_mut(),
                )
                .map_err(|_e| MemError::PageTableMap)? // TODO save inner error
                .flush();
        }

        let virt_base = page.start_address();
        debug!("Create apic base at addr: Phys {phys_base:p}, Virt {virt_base:p}");

        Ok(ApicBase(virt_base))
    }

    fn offset(&self, offset: Offset) -> VirtAddr {
        self.0 + offset as u64
    }

    fn id(&self) -> Volatile<&Id, ReadOnly> {
        let id = unsafe { self.offset(Offset::Id).as_volatile() };
        id
    }
}

fn disable_8259() {
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
    ExtendedApicFeature = 0x400,
    ExtendedApicControl = 0x410,
    SpecificEndOfInterrupt = 0x420,
    InterruptEnable = 0x480,
    ExtendedInterruptLocalVectorTable = 0x500,
}

#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct Id(u32);

impl Id {
    pub fn id(&self) -> u8 {
        self.0
            .get_bits(24..)
            .try_into()
            .expect("Apic id should fit into u8")
    }
}
