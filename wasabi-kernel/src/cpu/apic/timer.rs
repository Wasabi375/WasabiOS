//! The timer provided by the APIC
use bit_field::BitField;
use log::{info, trace};
use shared::{cpu::time::timestamp_now_tsc, types::TscTimestamp};

use crate::{
    cpu::interrupts::{self, InterruptFn, InterruptRegistrationError, InterruptVector},
    time::calibration_tick,
};

use super::{Apic, Offset, cpuid};

use core::ops::RangeInclusive;

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
pub struct TimerData {
    /// `true` if the timer is running at a constant speed
    ///
    /// This is hardware dependent
    constant_rate: bool,
    /// The clockspeed of the timer in mhz
    mhz: u64,
    /// th interrupt vector triggerd by the timer
    interrupt_vector: Option<InterruptVector>,
    /// the current [TimerMode]
    mode: TimerMode,
    /// the tsc time, when the timer was calibrated
    startup_tsc_time: TscTimestamp,
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
    pub(super) apic: &'a mut Apic,
}

impl core::fmt::Debug for Timer<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("").field(&self.apic.timer).finish()
    }
}

impl Timer<'_> {
    const MASK_BIT: usize = 16;
    const MODE_BITS: RangeInclusive<usize> = 17..=18;
    const DIVIDER_BITS: RangeInclusive<usize> = 0..=4;
    const VECTOR_BITS: RangeInclusive<usize> = 0..=7;

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
        // TODO how does is_running interact with one_shot timers
        // FIXME this is wrong in combination with enable_interrupt_handler
        //  this reports the timer as running, if a handler is registered
        //  even without calling start.
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
        vector: InterruptVector,
        handler: InterruptFn,
    ) -> Result<Option<InterruptVector>, InterruptRegistrationError> {
        interrupts::register_interrupt_handler(vector, handler)?;

        Ok(self.enable_interrupt_hander(vector))
    }

    /// unregister the current interrupt for the timer
    ///
    /// # Errors
    /// see [interrupts::unregister_interrupt_handler]
    pub fn unregister_interrupt_handler(
        &mut self,
    ) -> Result<Option<InterruptVector>, InterruptRegistrationError> {
        let old_vector = self.disable_interrupt_handler();
        let vector = self
            .apic
            .timer
            .interrupt_vector
            .ok_or(InterruptRegistrationError::NoRegisteredVector)?;

        interrupts::unregister_interrupt_handler(vector)?;

        Ok(old_vector)
    }

    /// enables an existing interrupt handler to handle timers
    pub fn enable_interrupt_hander(&mut self, vector: InterruptVector) -> Option<InterruptVector> {
        let old_vector = self.apic.timer.interrupt_vector.replace(vector);

        self.apic
            .offset_mut(Offset::TimerLocalVectorTableEntry)
            .update(|mut vte| {
                vte.set_bit(Timer::MASK_BIT, false);
                vte.set_bits(Timer::VECTOR_BITS, vector as u8 as u32);
                vte
            });

        old_vector
    }

    /// disables the currently enabled timer interrupt handler
    pub fn disable_interrupt_handler(&mut self) -> Option<InterruptVector> {
        log::trace!("disable timer handler");
        self.apic
            .offset_mut(Offset::TimerLocalVectorTableEntry)
            .update(|mut vte| {
                vte.set_bit(Timer::MASK_BIT, true);
                vte.set_bits(Timer::VECTOR_BITS, 0);
                vte
            });
        self.apic.timer.interrupt_vector.take()
    }

    /// starts the timer
    pub fn start(&mut self, mode: TimerMode) {
        let apic = &mut self.apic;
        match mode {
            TimerMode::Stopped => self.stop(),
            TimerMode::OneShot(config) | TimerMode::Periodic(config) => {
                let has_vector = apic.timer.interrupt_vector.is_some();
                if !has_vector {
                    log::warn!("start timer without vector!");
                }
                apic.offset_mut(Offset::TimerDivideConfiguration)
                    .update(|mut div| {
                        div.set_bits(Timer::DIVIDER_BITS, config.divider.into());
                        div
                    });
                apic.offset_mut(Offset::TimerLocalVectorTableEntry)
                    .update(|mut tlvte| {
                        tlvte.set_bit(Timer::MASK_BIT, !has_vector);
                        tlvte.set_bits(Timer::MODE_BITS, mode.vector_table_entry_bits());
                        tlvte
                    });
                assert_eq!(
                    apic.offset(Offset::TimerLocalVectorTableEntry)
                        .read()
                        .get_bit(Timer::MASK_BIT),
                    !has_vector
                );
                apic.offset_mut(Offset::TimerInitialCount)
                    .write(config.duration);
                apic.timer.mode = mode;
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

    /// debug logs the current timer register states
    pub fn debug_registers(&self) {
        let initial = self.apic.offset(Offset::TimerInitialCount).read();
        let current = self.apic.offset(Offset::TimerCurrentCount).read();
        let divide = self.apic.offset(Offset::TimerDivideConfiguration).read();

        log::debug!(
            "Apic Timer: Initial: {}, Current: {}, Divide: {:b}",
            initial,
            current,
            divide
        );
        let entry = self.apic.offset(Offset::TimerLocalVectorTableEntry).read();
        let mask = entry.get_bit(Timer::MASK_BIT);
        let mode = entry.get_bits(Timer::MODE_BITS);
        let divider = entry.get_bits(Timer::DIVIDER_BITS);
        let vector = entry.get_bits(Timer::VECTOR_BITS);

        log::debug!(
            "Apic Timer Entry: {}: Mask: {}, Mode: {}, Divider: {}, Vector: {}",
            entry,
            mask,
            mode,
            divider,
            vector
        );
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
    pub fn stop(&mut self) {
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

        let old_vector = self.enable_interrupt_hander(InterruptVector::Nop);

        self.apic.timer.startup_tsc_time = timestamp_now_tsc();
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

        if let Some(vector) = old_vector {
            self.enable_interrupt_hander(vector);
        } else {
            self.disable_interrupt_handler();
        }

        trace!(
            "timer {}, counted {} ticks in {} seconds",
            timer, elapsed_ticks, elapsed_seconds
        );

        // rate in mhz
        let rate = (elapsed_ticks as f64) / elapsed_seconds / 1_000_000.0;

        // round rate to nearest 100MHz and store it
        self.apic.timer.mhz = (((rate / 100.0) + 0.5) as u64) * 100;
    }
}
