#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod dispatch;
mod own_logger;
mod ref_logger;

#[cfg(feature = "color")]
use colored::Color;

pub use dispatch::DispatchLogger;
pub use own_logger::OwnLogger;
pub use ref_logger::RefLogger;

pub struct FlushError {}

pub trait TryLog: Send + Sync {
    fn enabled(&self, metadata: &log::Metadata) -> bool;

    fn log(&self, record: &log::Record) -> Result<(), core::fmt::Error>;

    fn flush(&self) -> Result<(), FlushError>;
}

pub trait LogSetup {
    fn with_module_rename(&mut self, target: &'static str, rename: &'static str) -> &mut Self;
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
