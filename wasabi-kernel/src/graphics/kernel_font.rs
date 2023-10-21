//! Functonality for a simple Kernel font
//!
//! This module does not include loading and working woth true-type-fonts, etc.
//! Instead it only contains functionality for loading and displaying a single
//! mono font (noto-sans-mono).

#[derive(Debug, Clone)]
pub struct BitFont {}

impl BitFont {
    pub fn line_height(&self) -> u32 {
        todo!()
    }
}
