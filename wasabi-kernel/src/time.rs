//! Functions for accessing TSC, apic timer, etc

use core::{
    hint::spin_loop,
    sync::atomic::{AtomicU64, Ordering},
};

use bit_field::BitField;
use x86_64::instructions::port::{Port, PortWriteOnly};

use shared::{
    cpu::time::{timestamp_now_tsc, timestamp_now_tsc_fenced},
    types::{Duration, TscDuration, TscTimestamp},
};

/// Spin loop until `duration` time has passed
///
/// The actual time spent might vary, if the tickrate is not constant.
pub fn sleep_tsc(duration: Duration) {
    let end = tsc_from_now(duration);
    while timestamp_now_tsc() < end {
        spin_loop();
    }
}

/// the value of rdtsc at the start of the kernels execution.
///
/// More preciesely this is the value of rdtsc at the time [calibrate_tsc]
/// was executed.
static STARTUP_TSC_TIME: AtomicU64 = AtomicU64::new(0);

/// the tsc tick rate in mhz
static TSC_MHZ: AtomicU64 = AtomicU64::new(3_000);

/// returns the value of rdtsc at startup
#[inline(always)]
pub fn tsc_startup() -> TscTimestamp {
    STARTUP_TSC_TIME.load(Ordering::Relaxed).into()
}

/// returns the number of tsc ticks since startup
#[inline(always)]
pub fn tsc_ticks_since_startup() -> TscDuration {
    timestamp_now_tsc() - tsc_startup()
}

/// returns the time since startup
#[inline(always)]
pub fn time_since_startup() -> Duration {
    Duration::new_micros((tsc_ticks_since_startup().as_i64() as f64 / tsc_tickrate() as f64) as u64)
}

/// returns the tsc tick rate in MHz
#[inline(always)]
pub fn tsc_tickrate() -> u64 {
    TSC_MHZ.load(Ordering::Relaxed)
}

/// calculates the time between 2 time stamps
///
/// This might not be accurate, if the tick rate is not constant.
#[inline(always)]
pub fn time_between_tsc<S, E>(start: S, end: E) -> Duration
where
    S: Into<TscTimestamp>,
    E: Into<TscTimestamp>,
{
    Duration::new_micros(
        ((end.into() - start.into()).as_i64() as f64 / tsc_tickrate() as f64) as u64,
    )
}

/// the expected number of tsc ticks, in `duration` time
#[inline(always)]
pub fn ticks_in(duration: Duration) -> TscDuration {
    (duration.to_micros().as_u64() * tsc_tickrate())
        .try_into()
        .expect("Tsc duration overflowed")
}

/// calculates the expected time stamp counter in `duration` time form now.
///
/// This might not be accurate, if the tick rate is not constant.
#[inline(always)]
pub fn tsc_from_now(duration: Duration) -> TscTimestamp {
    timestamp_now_tsc() + ticks_in(duration)
}

/// calculates the time betwee a timestamp and now
///
/// this assumes that `start` is a tsc before now. To calculate a duration
/// to a future time stamp use `time_between_tsc(read_tsc(), time_stamp)`
#[inline(always)]
pub fn time_since_tsc(start: TscTimestamp) -> Duration {
    Duration::new_micros(
        ((timestamp_now_tsc() - start).as_i64() as f64 / tsc_tickrate() as f64) as u64,
    )
}

/// using the pit, runs a "calibration tick" (One PIT channel 1 cycle).
///
/// This returns the time in seconds of the calibration tick.
///
/// This can be used to calibrate other timers.
/// ```no_run
/// # fn other_timer() -> u64 { todo!() }
/// let start = other_timer();
/// calibrate_tick();
/// let end = other_timer();
/// let rate_mhz = ((end - start) as f64) / elapsed_seconds / 1_000_000.0;
/// ```
///
/// TODO on systems with a known tsc tick rate (see cpuid 0x15) we can use that instead
///     as it is probably more acurate and definitely not less.
///     Not sure that I care, because my PC does not have that set
#[inline(always)]
pub fn calibration_tick() -> f64 {
    let mut pit_ctrl = PortWriteOnly::<u8>::new(0x43);
    let mut pit_data = Port::<u8>::new(0x40);

    unsafe {
        // https://wiki.osdev.org/Pit
        // set pit channel 1 to mode 0
        pit_ctrl.write(0b00110000);
        // set reload count to 65535
        // the write to the most significant byte starts the countdown.
        pit_data.write(0xff); // least significant
        pit_data.write(0xff); // most significant

        loop {
            // issue readback command so we can read the timer
            pit_ctrl.write(0b11100010);

            // break if the pit output pin is set
            if pit_data.read().get_bit(7) {
                break;
            }
        }
    }
    // compute the elapsed time in seconds. This is the number of ticks / (ticks / s)
    65535f64 / 1_193_182f64
}

/// calibrate the tsc tick rate and store it. It can be accessed by [tsc_tickrate]
pub fn calibrate_tsc() {
    STARTUP_TSC_TIME.store(timestamp_now_tsc().into(), Ordering::Relaxed);

    let start = timestamp_now_tsc_fenced();
    let elapsed_seconds = calibration_tick();
    let end = timestamp_now_tsc_fenced();

    // calculate the tsc rate in mhz
    let rate_mhz = ((end - start).as_i64() as f64) / elapsed_seconds / 1_000_000.0;

    // round to the nearest 100 Mhz, pit isn't precise enough for anything else
    let rate_mhz = (((rate_mhz / 100.0) + 0.5) as u64) * 100;

    TSC_MHZ.store(rate_mhz, Ordering::Relaxed);
}
