//! Utility functions for dealing with the `cpuid` instruction.
use bit_field::BitField;
use const_default::ConstDefault;
use core::{
    arch::x86_64::{__cpuid, __cpuid_count},
    ops::{Deref, DerefMut},
    ptr::{addr_of, addr_of_mut},
};
use log::{info, trace, warn};
use x86_64::registers::{
    rflags::{self, RFlags},
    xcontrol::XCr0Flags,
};

pub use core::arch::x86_64::CpuidResult;

/// calls the cpuid instruction with the provided `leaf` and if set `sub_leaf`
pub fn cpuid(leaf: u32, sub_leaf: Option<u32>) -> CpuidResult {
    // safety: cpuid is a save instruction, assuming [check_cpuid_usable]
    // succeeded
    if let Some(ecx) = sub_leaf {
        unsafe { __cpuid_count(leaf, ecx) }
    } else {
        unsafe { __cpuid(leaf) }
    }
}

/// get local_apic id by calling [cpuid].
pub fn apic_id() -> u8 {
    let cpuid_apci_info = cpuid(1, None);
    let local_apic_id: u8 = cpuid_apci_info
        .ebx
        .get_bits(24..)
        .try_into()
        .unwrap_or_else(|_| panic!("local apic id does not fit in u8"));

    local_apic_id
}

/// ensures that we are allowed to call [cpuid].
///
/// This will fail with an invalid opcode exception (#UD) if we are not
/// running on a valid intel cpu that supports apic.
///
/// # Safety
///
/// This can always fault, depending on the hardware. No way around it.
pub unsafe fn check_cpuid_usable() {
    trace!("check cpuid usable");

    let mut rflags = rflags::read();
    // assert!(rflags.contains(RFlags::ID), "expected ID in rflags");

    // cpuid works if we can toggle the id rflag
    rflags.toggle(RFlags::ID);
    // Safety: writing ID flag is safe(ish)
    //
    //      this will succede if we are running on a valid intel cpu
    //      that supports apic
    //
    //      if not this will cause a invalid opcode exception (#UD)
    unsafe {
        rflags::write(rflags);
        // rflags.toggle(RFlags::ID);
        // rflags::write(rflags);
    }

    // test for genuine intel cpu
    let genuine_intel = cpuid(0, None);

    trace!("Max value for cpuid eax: {}", genuine_intel.eax);

    const VALID_GENUINE: &[[&[u8; 4]; 3]] =
        &[[b"Genu", b"ntel", b"ineI"], [b"Auth", b"cAMD", b"enti"]];

    let valid_genuine = VALID_GENUINE.iter().any(|expected| {
        genuine_intel.ebx.to_ne_bytes() == *expected[0]
            && genuine_intel.ecx.to_ne_bytes() == *expected[1]
            && genuine_intel.edx.to_ne_bytes() == *expected[2]
    });
    if !valid_genuine {
        panic!(
            "Expected \"GenuineIntel\" or \"AuthenticAMD\" but got \"{}{}{}\" instead",
            genuine_intel
                .ebx
                .to_ne_bytes()
                .as_ascii()
                .map(|a| a.as_str())
                .unwrap_or("????"),
            genuine_intel
                .edx
                .to_ne_bytes()
                .as_ascii()
                .map(|a| a.as_str())
                .unwrap_or("????"),
            genuine_intel
                .ecx
                .to_ne_bytes()
                .as_ascii()
                .map(|a| a.as_str())
                .unwrap_or("????")
        )
    }

    trace!("cpuid usable");
}

#[derive(Debug, ConstDefault)]
/// Contains general information about the capabilites of the CPU
#[expect(missing_docs)]
pub struct CPUCapabilities {
    /// set to true if this struct is filled
    pub is_initialized: bool,
    /// the FXSAVE and FXRSTOR instructions are supported
    pub fxsave: bool,
    pub sse: bool,
    pub sse2: bool,
    pub sse3: bool,
    pub ssse3: bool,
    pub sse4_1: bool,
    pub sse4_2: bool,
    pub avx: bool,
    pub avx2: bool,
    /// true if the cs and ds selectors of the fpu are deprecated
    pub fpu_cs_ds_deprecated: bool,

    pub xsave: bool,
    pub xsave_features: XCr0FlagsW,
    pub xsave_area_max_size: u32,
    pub osxsave: bool,
    pub xsaveopt: bool,
    /// compacting extension for xsave feature set
    pub xsave_compact: bool,
}

/// Wrapper around [XCr0Flags] which also implements [ConstDefault]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct XCr0FlagsW(XCr0Flags);

impl Deref for XCr0FlagsW {
    type Target = XCr0Flags;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for XCr0FlagsW {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl core::fmt::Debug for XCr0FlagsW {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}
impl ConstDefault for XCr0FlagsW {
    const DEFAULT: Self = Self(XCr0Flags::empty());
}
impl From<XCr0Flags> for XCr0FlagsW {
    fn from(value: XCr0Flags) -> Self {
        Self(value)
    }
}
impl From<XCr0FlagsW> for XCr0Flags {
    fn from(value: XCr0FlagsW) -> Self {
        value.0
    }
}

static mut CPU_CAPABILITIES: CPUCapabilities = <CPUCapabilities as ConstDefault>::DEFAULT;

impl CPUCapabilities {
    /// Returns the [CPUCapabilities] querried during startup using the [cpuid] command
    pub fn get<'a>() -> &'a Self {
        // Safety: no mut reference held, because `load` is not called while this
        // reference exists
        unsafe { &*addr_of!(CPU_CAPABILITIES) }
    }

    /// Loads basic information about the cpu and stores it in [KernelData]
    ///
    /// # Safety
    ///
    /// Must be called once after [check_cpuid_usable] succeded and before any
    /// other cores are started.
    /// Caller must also ensure there are no references to [Self::get]
    pub unsafe fn load() {
        let mut cap = CPUCapabilities::DEFAULT;

        cap.is_initialized = true;

        let basic_features = cpuid(0x1, None);
        cap.fxsave = basic_features.edx.get_bit(24);
        cap.sse = basic_features.edx.get_bit(25);
        cap.sse2 = basic_features.edx.get_bit(26);

        cap.sse3 = basic_features.ecx.get_bit(0);
        cap.ssse3 = basic_features.ecx.get_bit(9);
        cap.sse4_1 = basic_features.ecx.get_bit(19);
        cap.sse4_2 = basic_features.ecx.get_bit(20);
        cap.avx = basic_features.ecx.get_bit(28);

        cap.xsave = basic_features.ecx.get_bit(26);
        cap.osxsave = basic_features.ecx.get_bit(27);

        let xsave_state = cpuid(0xd, None);
        let xsave_features = xsave_state.eax as u64 | ((xsave_state.edx as u64) << 32);
        cap.xsave_features = XCr0Flags::from_bits_truncate(xsave_features).into();
        if cap.xsave_features.bits() != xsave_features {
            warn!(
                "unknown xsave_feature bits in cpuid. All {:#x}, unknown {:#x}",
                xsave_features,
                xsave_features & !cap.xsave_features.bits()
            );
        }
        cap.xsave_area_max_size = xsave_state.ecx;
        let xsave_state_1 = cpuid(0xd, Some(1));
        cap.xsaveopt = xsave_state_1.eax.get_bit(0);
        cap.xsave_compact = xsave_state_1.eax.get_bit(1);

        let extended_features = cpuid(0x7, None);
        cap.fpu_cs_ds_deprecated = extended_features.ebx.get_bit(13);
        cap.avx2 = extended_features.ebx.get_bit(5);

        info!("{:#?}", cap);
        // Safety: no references to CPU_CAPABILITES exist and we are in a single core envirnoment
        unsafe {
            *addr_of_mut!(CPU_CAPABILITIES) = cap;
        }
    }
}
