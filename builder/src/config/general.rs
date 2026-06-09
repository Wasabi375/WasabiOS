use std::{ffi::OsStr, fmt::Display, result::Result as StdResult};

use congen::{
    Configuration,
    internal::{
        ChangeVerb, CongenChange, CongenInternal, FieldDescription, NotSupported, ParseError,
        VerbError,
    },
};
use serde::{
    Deserialize, Serialize,
    de::{self, Visitor},
};

#[derive(Debug, Clone, Copy)]
pub struct SizeBytes(u64);

#[derive(Debug, Clone, Copy)]
#[repr(usize)]
pub enum SizeMagnitude {
    Tera = 0,
    Giga,
    Mega,
    Kilo,
    Bytes,
}

impl SizeMagnitude {
    pub fn postfix(self) -> &'static str {
        match self {
            SizeMagnitude::Tera => "TB",
            SizeMagnitude::Giga => "GB",
            SizeMagnitude::Mega => "MB",
            SizeMagnitude::Kilo => "KB",
            SizeMagnitude::Bytes => "B",
        }
    }

    pub fn variants() -> [Self; 5] {
        use SizeMagnitude::*;
        [Tera, Giga, Mega, Kilo, Bytes]
    }
}

impl SizeBytes {
    #[allow(unused)]
    pub const ZERO: Self = Self::new(0);

    #[allow(unused)]
    pub const fn new(bytes: u64) -> Self {
        SizeBytes(bytes)
    }

    #[allow(unused)]
    pub const fn new_mb(mb: u64) -> Self {
        SizeBytes(mb * 1024 * 1024)
    }

    /// split `SizeBytes` into [SizeMagnitude] multipliers
    ///
    /// The order is from "TeraBytes" to "Bytes" which is also reflected in `size_magitude as usize`
    /// and [SizeMagnitude::variants].
    ///
    /// # Examples
    ///
    /// Get the number of kilobytes
    /// ```
    /// # use crate::config::general::{SizeBytes, SizeMagnitute};
    /// let size = SizeBytes(1052);
    /// assert_eq!(size.get_magnitudes()[SizeMagnitude::Kilo as usize], 1);
    /// ```
    pub fn get_magnitudes(self) -> [u64; 5] {
        let rest = self.0;
        let bytes = rest % 1024;
        let rest = rest / 1024;
        let kilo = rest % 1024;
        let rest = rest / 1024;
        let mega = rest % 1024;
        let rest = rest / 1024;
        let giga = rest % 1024;
        let tera = rest / 1024;
        [tera, giga, mega, kilo, bytes]
    }

    pub fn bytes(self) -> u64 {
        self.0
    }

    fn parser() -> parse_size::Config {
        parse_size::Config::new().with_binary()
    }
}

impl Display for SizeBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 == 0 {
            return f.write_str("0B");
        }
        let multis = self.get_magnitudes();
        for mag in SizeMagnitude::variants() {
            let multi = multis[mag as usize];
            if multi != 0 {
                f.write_fmt(format_args!("{} {}", multi, mag.postfix()))?;
            }
        }
        Ok(())
    }
}

impl Serialize for SizeBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SizeBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SizeVisitor;
        impl<'de> Visitor<'de> for SizeVisitor {
            type Value = SizeBytes;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("(<number>mag)+ pattern for parse-size")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> StdResult<SizeBytes, E> {
                match SizeBytes::parser().parse_size(value) {
                    Ok(size) => Ok(SizeBytes(size)),
                    Err(err) => Err(de::Error::custom(format!("failed to parse size: {err}"))),
                }
            }
        }

        deserializer.deserialize_str(SizeVisitor)
    }
}

impl Configuration for SizeBytes {}
impl CongenInternal for SizeBytes {
    type CongenChange = SizeBytesChange;

    fn apply_change_with_inner_default(
        &mut self,
        change: Self::CongenChange,
        _inner_default: Option<fn() -> Box<dyn std::any::Any>>,
    ) {
        if let Some(value) = change.0 {
            *self = value;
        }
    }

    fn description(field_name: &'static str) -> congen::internal::Description {
        FieldDescription {
            field_name,
            type_name: Self::type_name(),
            is_flag: false,
            allow_unset: false,
            has_default: false,
            clap_data: None,
        }
        .into()
    }
}

#[derive(Debug, Default, Clone)]
pub struct SizeBytesChange(Option<SizeBytes>);
impl CongenChange for SizeBytesChange {
    type Configuration = SizeBytes;

    fn empty() -> Self {
        SizeBytesChange(None)
    }

    fn apply_change(&mut self, change: Self) {
        if let Some(value) = change.0 {
            *self = SizeBytesChange(Some(value))
        }
    }

    fn parse(input: &OsStr) -> Result<Result<Self, ParseError>, NotSupported> {
        let input = match input.to_str() {
            Some(input) => input,
            None => {
                return Ok(Err(ParseError(format!(
                    "expected utf8 string but got {input:?}"
                ))));
            }
        };

        match SizeBytes::parser().parse_size(input) {
            Ok(value) => Ok(Ok(SizeBytesChange(Some(SizeBytes(value))))),
            Err(err) => Ok(Err(ParseError(format!("failed to parse size: {err}")))),
        }
    }

    fn from_path_and_verb<'a, P>(
        mut path: P,
        verb: congen::internal::ChangeVerb,
    ) -> StdResult<Self, congen::internal::VerbError>
    where
        P: Iterator<Item = &'a str>,
    {
        assert!(
            path.next().is_none(),
            "SizeBytesChange implies this is a field"
        );

        match verb {
            ChangeVerb::Set(os_string) => Ok(Self::parse(&os_string)??),
            ChangeVerb::SetAny(value) => {
                Ok(*value.downcast().map_err(|_| VerbError::DowncastFailed)?)
            }
            ChangeVerb::UseDefault
            | ChangeVerb::SetFlag
            | ChangeVerb::Unset
            | ChangeVerb::List(_) => Err(VerbError::UnsupportedVerb(verb)),
        }
    }
}
