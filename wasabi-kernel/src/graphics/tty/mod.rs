//! Contains strutcts and utilities related to drawing a tty and
//! logging environment
pub mod color;
pub use color::TextColor;
pub use color::TextColorError;

mod sgr;
pub use sgr::AnsiSGR;
pub use sgr::SGRParseError;
