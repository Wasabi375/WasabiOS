pub mod color;
pub use color::TextColor;
pub use color::TextColorError;

mod sgr;
pub use sgr::AnsiSGR;
pub use sgr::SGRParseError;
