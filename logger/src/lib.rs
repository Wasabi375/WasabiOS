#![no_std]

pub mod dispatch;
mod static_logger;

#[cfg(feature = "color")]
use colored::Color;

pub use dispatch::DispatchLogger;
pub use static_logger::StaticLogger;

pub struct FlushError {}

pub trait TryLog: Send + Sync {
    fn enabled(&self, metadata: &log::Metadata) -> bool;

    fn log(&self, record: &log::Record) -> Result<(), core::fmt::Error>;

    fn flush(&self) -> Result<(), FlushError>;
}

/// Default colors for logging
///
/// Indexed by `level as usize`: Text, Error, Warn, Info, Debug, Trace
/// See [log::Level]
pub const fn default_colors() -> [Color; 6] {
    [
        Color::White,
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::Blue,
        Color::White,
    ]
}
