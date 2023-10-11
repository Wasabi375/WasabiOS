//! Functions for accessing TSC, apic timer, etc

use core::{
    arch::x86_64::{_mm_lfence, _mm_mfence, _rdtsc},
    fmt::{Display, Write},
    hint::spin_loop,
    sync::atomic::{AtomicU64, Ordering},
};

use bit_field::BitField;
use x86_64::instructions::port::{Port, PortWriteOnly};

/// read the time stamp counter
///
/// The RDTSC instruction is not a  serializing instruction. It does not
/// necessarily wait until all previous instructions have been executed before
/// reading the counter. Similarly, subsequent instructions may begin execution
/// before the read operation is performed.
///
/// Use [read_tsc_fenced] if instruction order is important, however this
/// is less performant.
pub fn read_tsc() -> u64 {
    unsafe { _rdtsc() }
}

/// read the time stamp counter
///
/// This also ensures that all previous stores and loads are globaly visible
/// before reading tsc. And that tsc is read, before any following operation
/// is executed.  
/// If this is not strictly necessray [read_tsc] should be used instead as it
/// allows for better performance.
///
/// This is the same as
/// ```no_run
/// _mm_lfence();
// _mm_mfence();
/// let tsc = _rdtsc();
/// _mm_lfence();
/// tsc
/// ```
///
/// See [_mm_lfence], [_mm_mfence]
pub fn read_tsc_fenced() -> u64 {
    unsafe {
        _mm_lfence();
        _mm_mfence();
        let tsc = _rdtsc();
        _mm_lfence();
        tsc
    }
}

/// Spin loop until `duration` time has passed
///
/// The actual time spent might vary, if the tickrate is not constant.
pub fn sleep_tsc(duration: Duration) {
    let start = read_tsc();
    let end = start + ticks_in(duration);
    while read_tsc() < end {
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
pub fn tsc_startup() -> u64 {
    STARTUP_TSC_TIME.load(Ordering::Relaxed)
}

/// returns the number of tsc ticks since startup
#[inline(always)]
pub fn tsc_ticks_since_startup() -> u64 {
    read_tsc() - tsc_startup()
}

/// returns the time since startup
#[inline(always)]
pub fn time_since_startup() -> Duration {
    Duration::new_micros((tsc_ticks_since_startup() as f64 / tsc_tickrate() as f64) as u64)
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
pub fn time_between_tsc(start: u64, end: u64) -> Duration {
    Duration::new_micros(((end - start) as f64 / tsc_tickrate() as f64) as u64)
}

/// the expected number of tsc ticks, in `duration` time
#[inline(always)]
pub fn ticks_in(duration: Duration) -> u64 {
    duration.to_micros().as_u64() * tsc_tickrate()
}

/// calculates the expected time stamp counter in `duration` time form now.
///
/// This might not be accurate, if the tick rate is not constant.
#[inline(always)]
pub fn tsc_from_now(duration: Duration) -> u64 {
    read_tsc() + ticks_in(duration)
}

/// calculates the time betwee a timestamp and now
///
/// this assumes that `start` is a tsc before now. To calculate a duration
/// to a future time stamp use `time_between_tsc(read_tsc(), time_stamp)`
#[inline(always)]
pub fn time_since_tsc(start: u64) -> Duration {
    Duration::new_micros(((read_tsc() - start) as f64 / tsc_tickrate() as f64) as u64)
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
    STARTUP_TSC_TIME.store(read_tsc(), Ordering::Relaxed);

    let start = read_tsc_fenced();
    let elapsed_seconds = calibration_tick();
    let end = read_tsc_fenced();

    // calculate the tsc rate in mhz
    let rate_mhz = ((end - start) as f64) / elapsed_seconds / 1_000_000.0;

    // round to the nearest 100 Mhz, pit isn't precise enough for anything else
    let rate_mhz = (((rate_mhz / 100.0) + 0.5) as u64) * 100;

    TSC_MHZ.store(rate_mhz, Ordering::Relaxed);
}

/// an enum represention a timed duration
///
/// this does not support fractions so converting into a
/// larger scale will lose information.
#[derive(Clone, Copy)]
pub enum Duration {
    /// duration in microseconds
    Micros(u64),
    /// duration in milliseconds
    Millis(u64),
    /// duration in seconds
    Seconds(u64),
    /// duration in minutse
    Minutes(u64),
    /// duration in hours
    Hours(u64),
}

impl Duration {
    /// scale multiplier for micro seconds
    const MICROS_MUL: u64 = 1;
    /// scale multiplier for milli seconds
    const MILLIS_MUL: u64 = 1000;
    /// scale multiplier for seconds
    const SECONDS_MUL: u64 = 1_000_000;
    /// scale multiplier for minutes
    const MINUTES_MUL: u64 = 60_000_000;
    /// scale multiplier for hours
    const HOURS_MUL: u64 = 24 * Duration::MINUTES_MUL;

    /// creates a new [Duration] in micro seconds
    pub const fn new_micros(micros: u64) -> Self {
        Duration::Micros(micros)
    }

    /// creates a new [Duration] in milli seconds
    pub const fn new_millis(millis: u64) -> Self {
        Duration::Millis(millis)
    }

    /// creates a new [Duration] in seconds
    pub const fn new_seconds(seconds: u64) -> Self {
        Duration::Seconds(seconds)
    }

    /// creates a new [Duration] in minutes
    pub const fn new_minutes(minutes: u64) -> Self {
        Duration::Minutes(minutes)
    }

    /// creates a new [Duration] in hours
    pub const fn new_hours(hours: u64) -> Self {
        Duration::Hours(hours)
    }

    /// returns the internally used multiplier for conversions
    #[inline]
    const fn multiplier(&self) -> u64 {
        match self {
            Duration::Micros(_) => Duration::MICROS_MUL,
            Duration::Millis(_) => Duration::MILLIS_MUL,
            Duration::Seconds(_) => Duration::SECONDS_MUL,
            Duration::Minutes(_) => Duration::MINUTES_MUL,
            Duration::Hours(_) => Duration::HOURS_MUL,
        }
    }

    /// returns the si name suffix
    #[inline]
    const fn multiplier_name(&self) -> &'static str {
        match self {
            Duration::Micros(_) => {
                if cfg!(feature = "no-unicode-log") {
                    "mys"
                } else {
                    "\u{b5}s" // μs
                }
            }
            Duration::Millis(_) => "ms",
            Duration::Seconds(_) => "s",
            Duration::Minutes(_) => "m",
            Duration::Hours(_) => "h",
        }
    }

    /// returns the smaller multiplier of the 2 durations
    #[inline]
    const fn smallest_multiplier(a: Duration, b: Duration) -> u64 {
        if a.multiplier() < b.multiplier() {
            a.multiplier()
        } else {
            b.multiplier()
        }
    }

    /// returns both durations with the smaller muliplier, used for conversions
    #[inline]
    const fn with_smaller_multiplier(a: Duration, b: Duration) -> (Option<u64>, Option<u64>) {
        let target_multi = Duration::smallest_multiplier(a, b);
        (
            a.as_u64().checked_mul(a.multiplier() / target_multi),
            b.as_u64().checked_mul(b.multiplier() / target_multi),
        )
    }

    /// get's the duration u64 in it's current scale.
    /// use `to_micors().as_u64()` to get a specific scale.
    #[inline]
    pub const fn as_u64(&self) -> u64 {
        match self {
            Duration::Micros(x) => *x,
            Duration::Millis(x) => *x,
            Duration::Seconds(x) => *x,
            Duration::Minutes(x) => *x,
            Duration::Hours(x) => *x,
        }
    }

    /// converts duration into micro seconds
    #[inline]
    pub const fn to_micros(&self) -> Duration {
        Duration::new_micros(self.as_u64() * self.multiplier() / Duration::MICROS_MUL)
    }

    /// converts duration into milli seconds
    #[inline]
    pub const fn to_millis(&self) -> Duration {
        Duration::new_millis(self.as_u64() * self.multiplier() / Duration::MILLIS_MUL)
    }

    /// converts duration into seconds
    #[inline]
    pub const fn to_seconds(&self) -> Duration {
        Duration::new_seconds(self.as_u64() * self.multiplier() / Duration::SECONDS_MUL)
    }

    /// converts duration into minutes.
    #[inline]
    pub const fn to_minutes(&self) -> Duration {
        Duration::new_minutes(self.as_u64() * self.multiplier() / Duration::MINUTES_MUL)
    }

    /// converts duration into hours
    #[inline]
    pub const fn to_hours(&self) -> Duration {
        Duration::new_hours(self.as_u64() * self.multiplier() / Duration::HOURS_MUL)
    }

    /// converts duration into seconds, returning a [f64].
    /// Unlike the [Duration::to_seconds] this will keep fractional seconds
    #[inline]
    pub fn as_seconds(&self) -> f64 {
        ((self.as_u64() as f64) * self.multiplier() as f64) / (Duration::SECONDS_MUL as f64)
    }
}

impl PartialEq for Duration {
    fn eq(&self, other: &Self) -> bool {
        match Duration::with_smaller_multiplier(*self, *other) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Duration {}

impl PartialOrd for Duration {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Duration {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        match Duration::with_smaller_multiplier(*self, *other) {
            (Some(a), Some(b)) => a.cmp(&b),
            // other is to large to fit into the smaler scale,
            // therefor it must be larger than self
            (Some(_), None) => core::cmp::Ordering::Less,
            // self is to large to fit into the smaler scale,
            // therefor it must be larger than other
            (None, Some(_)) => core::cmp::Ordering::Greater,
            (None, None) => core::cmp::Ordering::Equal,
        }
    }
}

impl core::fmt::Debug for Duration {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("{}{}", self.as_u64(), self.multiplier_name()))
    }
}

impl Display for Duration {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.as_u64() == 0 {
            return f.write_fmt(format_args!("{self:?}"));
        }

        let mut scaled = self.as_u64() * self.multiplier();

        let mut write_rest = false;

        let mut unit_type = None;

        if scaled >= Duration::HOURS_MUL {
            let hours = scaled / Duration::HOURS_MUL;
            f.write_fmt(format_args!("{:0>2}:", hours))?;
            scaled -= hours * Duration::HOURS_MUL;
            write_rest = true;
            unit_type = Some(Duration::new_hours(0).multiplier_name());
        }

        if scaled >= Duration::MINUTES_MUL || write_rest {
            let minutes = scaled / Duration::MINUTES_MUL;
            f.write_fmt(format_args!("{:0>2}:", minutes))?;
            scaled -= minutes * Duration::MINUTES_MUL;
            write_rest = true;
            unit_type = unit_type.or(Some(Duration::new_minutes(0).multiplier_name()));
        }

        if scaled >= Duration::SECONDS_MUL || write_rest {
            let seconds = scaled / Duration::SECONDS_MUL;
            f.write_fmt(format_args!("{:0>2}.", seconds))?;
            scaled -= seconds * Duration::SECONDS_MUL;
            write_rest = true;
            unit_type = unit_type.or(Some(Duration::new_seconds(0).multiplier_name()));
        }

        if (scaled > 0 && scaled >= Duration::MILLIS_MUL) || write_rest {
            let millis = scaled / Duration::MILLIS_MUL;
            f.write_fmt(format_args!("{:0>3}", millis))?;
            scaled -= millis * Duration::MILLIS_MUL;

            unit_type = unit_type.or(Some(Duration::new_millis(0).multiplier_name()));

            if scaled > 0 {
                f.write_char('_')?;
            }
        }

        if scaled > 0 {
            let micros = scaled / Duration::MICROS_MUL;
            f.write_fmt(format_args!("{:0>3}", micros))?;
            scaled -= micros * Duration::MICROS_MUL;

            unit_type = unit_type.or(Some(Duration::new_micros(0).multiplier_name()));
        }

        f.write_str(unit_type.expect("There must be a unit type, because value is not 0"))?;

        assert!(scaled == 0);

        Ok(())
    }
}
