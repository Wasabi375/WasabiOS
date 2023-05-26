use core::arch::x86_64::{__cpuid, __cpuid_count};
use log::trace;
use x86_64::registers::rflags::{self, RFlags};

pub use core::arch::x86_64::CpuidResult;

pub fn cpuid(leaf: u32, sub_leaf: Option<u32>) -> CpuidResult {
    if let Some(ecx) = sub_leaf {
        unsafe { __cpuid_count(leaf, ecx) }
    } else {
        unsafe { __cpuid(leaf) }
    }
}

pub fn check_cpuid_usable() {
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

    fn u32_assert(u: u32, str: &str) {
        let u_bytes: &[u8; 4] = unsafe { core::mem::transmute(&u) };
        // Safety: we don't care that the to_ascii conversion of bytes might fail
        // for printing.
        // Worst case we get a missing char symbol. Most terminals are utf8 anyways
        unsafe {
            assert!(
                u_bytes == str.as_bytes(),
                "expected \"{str}\" but got \"{}\" instaed",
                u_bytes.as_ascii_unchecked().as_str()
            );
        }
    }

    // This does not seem to work with qemu, but should work in real hardware
    // u32_assert(genuine_intel.ebx, r"Genu");
    // u32_assert(genuine_intel.ecx, r"ntel");
    // u32_assert(genuine_intel.ebx, r"ienI");
}
