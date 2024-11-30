use core::{
    fmt::{Display, Write},
    num::TryFromIntError,
    ops::{Add, Sub},
};

/// A timestamp based on the tsc counter
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TscTimestamp(u64);

impl TscTimestamp {
    /// Create a new timestamp
    pub const fn new(t: u64) -> Self {
        Self(t)
    }

    /// convert the timsetamp to a u64
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for TscTimestamp {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

impl From<TscTimestamp> for u64 {
    fn from(value: TscTimestamp) -> Self {
        value.as_u64()
    }
}

impl Sub<TscTimestamp> for TscTimestamp {
    type Output = TscDuration;

    fn sub(self, rhs: TscTimestamp) -> Self::Output {
        let slf = self.0 as i128;
        let rhs = rhs.0 as i128;
        (slf - rhs)
            .try_into()
            .expect("Duration does not fit within i64 ticks")
    }
}

/// A duration based on the tsc counter
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TscDuration(i64);

impl TscDuration {
    /// create a new duration
    pub const fn new(t: i64) -> Self {
        Self(t)
    }

    /// convert the duration to an i64
    pub const fn as_i64(self) -> i64 {
        self.0
    }
}

impl From<i64> for TscDuration {
    fn from(value: i64) -> Self {
        Self::new(value)
    }
}

impl TryFrom<u64> for TscDuration {
    type Error = TryFromIntError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(Self::new(value.try_into()?))
    }
}

impl TryFrom<i128> for TscDuration {
    type Error = TryFromIntError;

    fn try_from(value: i128) -> Result<Self, Self::Error> {
        Ok(Self::new(value.try_into()?))
    }
}

impl From<TscDuration> for i64 {
    fn from(value: TscDuration) -> Self {
        value.as_i64()
    }
}

impl Add<TscDuration> for TscDuration {
    type Output = TscDuration;

    fn add(self, rhs: TscDuration) -> Self::Output {
        self.as_i64()
            .checked_add(rhs.as_i64())
            .expect("Tsc duration overflowed")
            .into()
    }
}

impl Sub<TscDuration> for TscDuration {
    type Output = TscDuration;

    fn sub(self, rhs: TscDuration) -> Self::Output {
        self.as_i64()
            .checked_sub(rhs.as_i64())
            .expect("Tsc duration overflowed")
            .into()
    }
}

impl Add<TscDuration> for TscTimestamp {
    type Output = TscTimestamp;

    fn add(self, rhs: TscDuration) -> Self::Output {
        let tsc = (self.as_u64() as i128) + (rhs.as_i64() as i128);
        let tsc: u64 = tsc.try_into().expect("Tsc timestamp overflowed");
        tsc.into()
    }
}

impl Sub<TscDuration> for TscTimestamp {
    type Output = TscTimestamp;

    fn sub(self, rhs: TscDuration) -> Self::Output {
        let tsc = (self.as_u64() as i128) - (rhs.as_i64() as i128);
        let tsc: u64 = tsc.try_into().expect("Tsc timestamp overflowed");
        tsc.into()
    }
}

/// an enum represention a timed duration
///
/// this does not support fractions so converting into a
/// larger scale will lose information.
// TODO why do I have my own duration class
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

        if (scaled >= Duration::MILLIS_MUL) || write_rest {
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
