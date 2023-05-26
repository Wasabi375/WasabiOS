use core::arch::asm;

use x86_64::instructions;

pub mod cpuid;
pub mod gdt;
pub mod interrupts;

#[inline]
pub fn halt_single() {
    instructions::hlt();
}

#[inline]
pub fn halt() -> ! {
    loop {
        halt_single();
    }
}

#[inline]
pub unsafe fn read_rip() -> u64 {
    let rdi: u64;
    asm! {

        "lea {0}, [rip]",
        out(reg) rdi
    }
    rdi
}
