//! Utility functions for dealing with the `cpuid` instruction.
use bit_field::BitField;
use core::arch::x86_64::{__cpuid, __cpuid_count};
use log::trace;
use x86_64::registers::rflags::{self, RFlags};

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
/// # Safety:
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

    // This does not seem to work with qemu, but should work in real hardware
    // fn u32_assert(u: u32, str: &str) {
    //     let u_bytes: &[u8; 4] = unsafe { core::mem::transmute(&u) };
    //     // Safety: we don't care that the to_ascii conversion of bytes might fail
    //     // for printing.
    //     // Worst case we get a missing char symbol. Most terminals are utf8 anyways
    //     unsafe {
    //         assert!(
    //             u_bytes == str.as_bytes(),
    //             "expected \"{str}\" but got \"{}\" instaed",
    //             u_bytes.as_ascii_unchecked().as_str()
    //         );
    //     }
    // }
    //
    // u32_assert(genuine_intel.ebx, r"Genu");
    // u32_assert(genuine_intel.ecx, r"ntel");
    // u32_assert(genuine_intel.ebx, r"ienI");
    trace!("cpuid usable");
}
