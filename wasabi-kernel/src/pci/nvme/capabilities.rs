//! NVMe Controller Capabilities
//!
//! This describes the capabilites supported by this kernel
//!
//! The specification documents can be found at https://nvmexpress.org/specifications/
//! specifically: NVM Express Base Specification and NVM Command Set Specification

use bitflags::bitflags;

use super::admin_commands::NamespaceFeatures;

/// The capabilities of a [super::NVMEController]
#[derive(Debug, Default, Clone)]
pub struct ControllerCapabilities {
    /// describes which commands can be fused
    pub fuses: Fuses,

    /// The page size used for physical memory pages
    pub memory_page_size: u32,

    pub optional_admin_commands: OptionalAdminCommands,

    pub lba_block_size: u64,
    pub namespace_size_blocks: u64,
    pub namespace_capacity_blocks: u64,
    pub namespace_features: NamespaceFeatures,
}

bitflags! {
    /// Bitflag describing the supported Fused NVME commands
    ///
    /// See: NVM Command Set Spec: 2.3.1 Fused Operations
    /// See: NVM Express Base Spec: Figure 276: Identify Controller Data Structure: FUSES
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    #[allow(missing_docs)]
    pub struct Fuses: u16 {
        const COMPARE_AND_WRITE = 1;
    }

    /// Bitflag describing the optional admin commands supported a controller
    ///
    /// See: NVM Express Base Spec: Figure 276: Identify Controller Data Structure: OACS
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    #[allow(missing_docs)]
    pub struct OptionalAdminCommands: u16 {
        const NAMESPACE_MANAGMENT = 1 << 3;
    }
}
