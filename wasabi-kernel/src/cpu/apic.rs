//! Local Apic implementation

// #![allow(missing_docs)] // TODO remove
use core::ops::RangeInclusive;

use crate::{
    cpu::cpuid::cpuid,
    locals, map_frame,
    mem::{MemError, VirtAddrExt},
    time::{calibration_tick, read_tsc},
};
use bit_field::BitField;
use log::{debug, info, trace};
use shared::lockcell::LockCell;
use thiserror::Error;
use volatile::{
    access::{ReadOnly, ReadWrite},
    Volatile,
};
use x86_64::{
    instructions::port::Port,
    registers::model_specific::Msr,
    structures::paging::{PageTableFlags, PhysFrame, Size4KiB},
    PhysAddr, VirtAddr,
};

use super::interrupts::{self, InterruptFn, InterruptRegistrationError};

/// MSR address of the local apic base.
const IA32_APIC_BASE: Msr = Msr::new(0x1b);

/// initializes the apic and stores it in [CoreLocals](crate::core_local::CoreLocals)
pub fn init() -> Result<(), ApicCreationError> {
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
    debug!("local apic id: {local_apic_id}");

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
    info!("Apic initialized");

    Ok(())
}

/// A struct representing a local apic.
pub struct Apic {
    /// the base vaddr of the Apic, used to access apic registers
    base: VirtAddr,

    timer: TimerData,
}

/// The different modes the apic timer can be in
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimerMode {
    /// the timer is not running
    Stopped,
    /// the timer is running and trigger a single interrupt at the end
    ///
    /// This will not automatically set the timer into stopped mode.  The timer
    /// will still be in OneShot mode but will not trigger a second interrupt.
    OneShot(TimerConfig),
    /// the timer is running and triggering interrupts on a regular basis
    Periodic(TimerConfig),
    /// the timer will trigger an interrupt when tsc reaches the deadline.
    ///
    /// Not implemented
    TscDeadline,
}

impl TimerMode {
    fn vector_table_entry_bits(&self) -> u32 {
        match self {
            TimerMode::Stopped => panic!("stopped mode can't be converted into entry bits"),
            TimerMode::OneShot(_) => 0b00,
            TimerMode::Periodic(_) => 0b01,
            TimerMode::TscDeadline => 0b10,
        }
    }
}

/// Configuration for the Apic timer.
///
/// The time between timer interrupts is
/// `duration / apic.timer().rate_mhz() * divider`
///
/// The divider can be used as a multiplier for duration to exceed the u32 size
///
/// # Example
///
/// to set the timer to interrupt every second any divider will work. Let's choose
/// [TimerDivider::DivBy2]. Therefor we have to set the duration to
/// `apic.timer().rate_mhz()  * 1_000_000 / 2`.
/// The timer rate is `rate_mhz() * 1_000_000` per second and the divier 2 means
/// that only every second timer flank causes a timer tick, making the timer take
/// twice as long. Therefor we divide the number of ticks per interrupt (duration)
/// by 2 to get back at 1 interrupt per second.
///
/// ## See
/// * [Timer::rate_mhz]
/// * [TimerDivider]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerConfig {
    /// specifies how often the timer ticks.
    ///
    /// The tickrate of the timer is `apic.timer().rate_mhz() / divider`
    /// See [Timer::rate_mhz]
    pub divider: TimerDivider,
    /// The number of timer ticks, after which the timer interrupt is called.
    pub duration: u32,
}

/// The divider slows down the clockspeed of the timer.
/// The base clockspeed can be queryed via [`apic.timer().rate_mhz()`]
///
/// [`apic.timer().rate_mhz()`]: Timer::rate_mhz
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerDivider {
    /// Clockspeed is unmodified
    DivBy1,
    /// Clockspeed is halfed
    DivBy2,
    /// Clockspeed is diveded by a factor of 4
    DivBy4,
    /// Clockspeed is diveded by a factor of 8
    DivBy8,
    /// Clockspeed is diveded by a factor of 16
    DivBy16,
    /// Clockspeed is diveded by a factor of 32
    DivBy32,
    /// Clockspeed is diveded by a factor of 64
    DivBy64,
    /// Clockspeed is diveded by a factor of 128
    DivBy128,
}

impl Into<u32> for TimerDivider {
    fn into(self) -> u32 {
        match self {
            TimerDivider::DivBy1 => 0b1011,
            TimerDivider::DivBy2 => 0b0000,
            TimerDivider::DivBy4 => 0b0001,
            TimerDivider::DivBy8 => 0b0010,
            TimerDivider::DivBy16 => 0b0011,
            TimerDivider::DivBy32 => 0b1000,
            TimerDivider::DivBy64 => 0b1001,
            TimerDivider::DivBy128 => 0b1010,
        }
    }
}

impl Default for TimerMode {
    fn default() -> Self {
        TimerMode::Stopped
    }
}

/// the current state of the timer
#[derive(Debug, Clone, Default)]
struct TimerData {
    /// `true` if the timer is running at a constant speed
    ///
    /// This is hardware dependent
    constant_rate: bool,
    /// The clockspeed of the timer in mhz
    mhz: u64,
    /// th interrupt vector triggerd by the timer
    interrupt_vector: Option<u8>,
    /// the current [TimerMode]
    mode: TimerMode,
    /// the tsc time, when the timer was calibrated
    startup_tsc_time: u64,
    /// `true` if the hardware supports deadline mode
    supports_tsc_deadline: bool,
}

/// A reference to the apic timer
///
/// There is always a single ApicTimer. This struct just provides access.
/// Dropping this does not stop the timer. Use [`stop`] instaed.
///
/// [`stop`]: Timer::stop
pub struct Timer<'a> {
    apic: &'a mut Apic,
}

impl core::fmt::Debug for Timer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("").field(&self.apic.timer).finish()
    }
}

/// The error used for Apic creation
#[derive(Error, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum ApicCreationError {
    #[error("{0}")]
    Mem(MemError),
    #[error("Invalid Physical Base {0:p}")]
    InvalidBase(PhysAddr),
}

impl Apic {
    /// create a [Apic] reading it's address from the [Msr]
    fn create_from_msr(apic_base: Msr) -> Result<Self, ApicCreationError> {
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

        let page = unsafe {
            // Safety: new page with apic frame (only used here) and is therefor safe
            map_frame!(Size4KiB, apic_table_flags, phys_frame)
                .map_err(|e| ApicCreationError::Mem(e))?
        };

        let virt_base = page.start_address();
        debug!("Create apic base at addr: Phys {phys_base:p}, Virt {virt_base:p}");

        let mut apic = Apic {
            base: virt_base,
            timer: TimerData::default(),
        };

        // stop the timer in case the bootloader used the apic timer
        apic.timer().stop();

        Ok(apic)
    }

    /// calculate [Volatile] for the given [Offset]
    fn offset(&self, offset: Offset) -> Volatile<&u32, ReadOnly> {
        let vaddr = self.base + offset as u64;
        // safety: we have read access to apic, so we can read it's registers
        unsafe { vaddr.as_volatile() }
    }

    /// calculate mutable [Volatile] for the given [Offset]
    fn offset_mut(&mut self, offset: Offset) -> Volatile<&mut u32, ReadWrite> {
        let vaddr = self.base + offset as u64;
        // safety: we have mut access to apic, so we can read and write it's registers
        unsafe { vaddr.as_volatile_mut() }
    }

    /// get access to the apic timer
    pub fn timer(&mut self) -> Timer {
        Timer { apic: self }
    }

    /// returns the [Id] of this [Apic]
    pub fn id(&self) -> Id {
        let id = self.offset(Offset::Id);
        Id(id.read())
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
            let apic = locals!().apic.get_mut().as_mut().unwrap();

            apic.offset_mut(Offset::EndOfInterrupt).write(0u32);
        }
    }
}

impl Timer<'_> {
    const MASK_BIT: usize = 16;
    const MODE_BITS: RangeInclusive<usize> = 17..=18;
    const DIVIDER_BITS: RangeInclusive<usize> = 0..=4;
    const VECTOR_BIST: RangeInclusive<usize> = 0..=7;

    /// returns `true` if the timer is running at a constant rate.
    ///
    /// This is hardware dependent
    pub fn constant_rate(&self) -> bool {
        self.apic.timer.constant_rate
    }

    /// the clockspeed of the timer in mhz
    pub fn rate_mhz(&self) -> u64 {
        self.apic.timer.mhz
    }

    /// `true` if the timer is running and can cause interrupts
    pub fn is_running(&self) -> bool {
        self.apic.timer.interrupt_vector.is_some()
    }

    /// `true` if the hardware supports tsc-deadline mode
    pub fn supports_tsc_deadline(&self) -> bool {
        self.apic.timer.supports_tsc_deadline
    }

    /// register an interrupt handler for the timer to call.
    ///
    /// # Errors
    /// see [interrupts::register_interrupt_handler]
    pub fn register_interrupt_handler(
        &mut self,
        vector: u8,
        handler: InterruptFn,
    ) -> Result<(), InterruptRegistrationError> {
        interrupts::register_interrupt_handler(vector, handler)?;

        let apic = &mut self.apic;
        apic.offset_mut(Offset::TimerLocalVectorTableEntry)
            .update(|vte| {
                vte.set_bit(Timer::MASK_BIT, false);
                vte.set_bits(Timer::VECTOR_BIST, vector as u32);
            });

        self.apic.timer.interrupt_vector = Some(vector);

        Ok(())
    }

    /// unregister the current interrupt for the timer
    ///
    /// # Errors
    /// see [interrupts::unregister_interrupt_handler]
    pub fn unregister_interrupt_handler(&mut self) -> Result<(), InterruptRegistrationError> {
        let apic = &mut self.apic;
        apic.offset_mut(Offset::TimerLocalVectorTableEntry)
            .update(|vte| {
                vte.set_bit(Timer::MASK_BIT, true);
                vte.set_bits(Timer::VECTOR_BIST, 0);
            });
        let vector = apic
            .timer
            .interrupt_vector
            .ok_or(InterruptRegistrationError::NoRegisteredVector)?;

        interrupts::unregister_interrupt_handler(vector)?;

        Ok(())
    }

    /// starts the timer
    pub fn start(&mut self, mode: TimerMode) {
        let apic = &mut self.apic;
        match mode {
            TimerMode::Stopped => self.stop(),
            TimerMode::OneShot(config) | TimerMode::Periodic(config) => {
                let has_vector = apic.timer.interrupt_vector.is_some();
                apic.offset_mut(Offset::TimerDivideConfiguration)
                    .update(|div| {
                        div.set_bits(Timer::DIVIDER_BITS, config.divider.into());
                    });
                apic.offset_mut(Offset::TimerLocalVectorTableEntry)
                    .update(|tlvte| {
                        tlvte.set_bit(Timer::MASK_BIT, !has_vector);
                        tlvte.set_bits(Timer::MODE_BITS, mode.vector_table_entry_bits());
                    });
                apic.offset_mut(Offset::TimerInitialCount)
                    .write(config.duration);
            }
            TimerMode::TscDeadline => {
                assert!(
                    self.apic.timer.supports_tsc_deadline,
                    "TscDeadline mode not supported"
                );
                todo!("implement tsc deadline mode");
            }
        }
    }

    /// restarts the timer.
    ///
    /// Resets the timer counter to `reset`. See [TimerConfig::duration]
    ///
    /// # Panics
    ///
    /// This will panic if the timer is not in Periodic or OneShot mode.
    pub fn restart(&mut self, reset: u32) {
        match self.apic.timer.mode {
            TimerMode::OneShot(_) | TimerMode::Periodic(_) => {
                self.apic.offset_mut(Offset::TimerInitialCount).write(reset);
            }
            _ => panic!(
                "restart only supported for OneShot and Periodic mode, not {:?}",
                self.apic.timer.mode
            ),
        }
    }

    /// Stops the apic timer
    fn stop(&mut self) {
        let apic = &mut self.apic;
        // set initial count to 0 to stop the timer
        apic.offset_mut(Offset::TimerInitialCount).write(0);

        apic.timer.mode = TimerMode::Stopped;
    }

    /// calibrates the apic timer rate based on the PIT timer.
    ///
    /// # Panics
    /// this will panic if the timer is not stopped or an interrupt vector is set
    ///
    /// #See
    /// [time::calibration_tick]
    pub fn calibrate(&mut self) {
        assert_eq!(self.apic.timer.mode, TimerMode::Stopped);
        assert!(self.apic.timer.interrupt_vector.is_none());

        info!("calibrating apic timer");

        self.apic.timer.startup_tsc_time = read_tsc();
        self.apic.timer.supports_tsc_deadline = cpuid(0x1, None).ecx.get_bit(24);

        self.apic.timer.constant_rate = cpuid(0x6, None).eax.get_bit(2);

        let calibration = TimerMode::OneShot(TimerConfig {
            divider: TimerDivider::DivBy1,
            duration: u32::MAX - 1,
        });

        // start the calibration timer
        self.start(calibration);

        // wait for elapsed seconds
        let elapsed_seconds = calibration_tick();

        // count ticks since timer start
        let timer = self.apic.offset(Offset::TimerCurrentCount).read();

        let elapsed_ticks = { u32::MAX - timer };

        // stop calibration timer, we don't need it anymore
        self.stop();

        info!(
            "timer {}, counted {} ticks in {} seconds",
            timer, elapsed_ticks, elapsed_seconds
        );

        // rate in mhz
        let rate = (elapsed_ticks as f64) / elapsed_seconds / 1_000_000.0;

        // round rate to nearest 100MHz and store it
        self.apic.timer.mhz = (((rate / 100.0) + 0.5) as u64) * 100;
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
