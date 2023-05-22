use x86_64::instructions;

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
