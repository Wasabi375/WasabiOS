use core::{
    fmt,
    ops::{Deref, DerefMut},
};

/// contains the id for a given core
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CoreId(pub u8);

impl CoreId {
    /// Whether this core is used as the bootstrap processor used for initialization of
    /// global systems
    pub fn is_bsp(&self) -> bool {
        self.0 == 0
    }
}

impl Deref for CoreId {
    type Target = u8;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for CoreId {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<u8> for CoreId {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

impl Into<u8> for CoreId {
    fn into(self) -> u8 {
        self.0
    }
}

impl fmt::Display for CoreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
