//! Functions for accessing TSC, apic timer, etc

use core::{
    arch::x86_64::{_mm_lfence, _mm_mfence, _rdtsc},
    fmt::Display,
};

/// read the time stamp counter
///
/// The RDTSC instruction is not a serializing instruction. It does not
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
/// _mm_mfence();
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
            #[cfg(not(feature = "no-unicode-log"))]
            Duration::Micros(_) => "\u{03bc}s", // μs
            #[cfg(feature = "no-unicode-log")]
            Duration::Micros(_) => "mys",
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
    pub const fn to_micros(&self) -> u64 {
        self.as_u64() * self.multiplier() / Duration::MICROS_MUL
    }

    /// converts duration into milli seconds
    #[inline]
    pub const fn to_millis(&self) -> u64 {
        self.as_u64() * self.multiplier() / Duration::MILLIS_MUL
    }

    /// converts duration into seconds
    #[inline]
    pub const fn to_seconds(&self) -> u64 {
        self.as_u64() * self.multiplier() / Duration::SECONDS_MUL
    }

    /// converts duration into minutes.
    #[inline]
    pub const fn to_minutes(&self) -> u64 {
        self.as_u64() * self.multiplier() / Duration::MINUTES_MUL
    }

    /// converts duration into hours
    #[inline]
    pub const fn to_hours(&self) -> u64 {
        self.as_u64() * self.multiplier() / Duration::HOURS_MUL
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

        if scaled >= Duration::HOURS_MUL {
            let hours = scaled / Duration::HOURS_MUL;
            f.write_fmt(format_args!("{:0>2}:", hours))?;
            scaled -= hours * Duration::HOURS_MUL;
            write_rest = true;
        }

        if scaled >= Duration::MINUTES_MUL || write_rest {
            let minutes = scaled / Duration::MINUTES_MUL;
            f.write_fmt(format_args!("{:0>2}:", minutes))?;
            scaled -= minutes * Duration::MINUTES_MUL;
            write_rest = true;
        }

        if scaled >= Duration::SECONDS_MUL || write_rest {
            let seconds = scaled / Duration::SECONDS_MUL;
            f.write_fmt(format_args!("{:0>2}.", seconds))?;
            scaled -= seconds * Duration::SECONDS_MUL;
            write_rest = true;
        }

        if scaled > 0 && (scaled >= Duration::MILLIS_MUL || write_rest) {
            let millis = scaled / Duration::MILLIS_MUL;
            f.write_fmt(format_args!("{:0>3}_", millis))?;
            scaled -= millis * Duration::MILLIS_MUL;
        }

        if scaled > 0 {
            let micros = scaled / Duration::MICROS_MUL;
            f.write_fmt(format_args!("{:0>3}_", micros))?;
            scaled -= micros * Duration::MICROS_MUL;
        }

        assert!(scaled == 0);

        Ok(())
    }
}
