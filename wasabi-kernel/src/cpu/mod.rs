//! A module containing cpu utilities as well as more specific sub modules.

pub mod apic;
pub mod cpuid;
pub mod gdt;
pub mod interrupts;

use core::arch::asm;
use x86_64::{instructions, registers::model_specific::Msr};

/// MSR for active FS base
static mut IA32_FS_BASE: Msr = Msr::new(0xc0000100);

/// MSR for active GS base
static mut IA32_GS_BASE: Msr = Msr::new(0xc0000101);

/// issues a single halt instruction
#[inline]
pub fn halt_single() {
    instructions::hlt();
}

/// issues the halt instruction in a loop.
#[inline]
pub fn halt() -> ! {
    loop {
        halt_single();
    }
}

/// reads the RIP register.
///
/// # Safety:
///
/// the caller must guarantee that it is save to access the register.
/// This should always be the case, but I still want this operation
/// to be marked as unsafe, because messing with the RIP is never a good idea.
/// Basically this should only be called to display additional debugging information.
#[inline]
pub unsafe fn read_rip() -> u64 {
    let rdi: u64;
    asm! {

        "lea {0}, [rip]",
        out(reg) rdi
    }
    rdi
}

/// Disbales interrupts.
///
/// When possibel `locals!().disbale_interrupts()` should be used instead.
///
/// ## See:
/// [crate::core_local::CoreLocals]  
/// [crate::locals]
///
/// # Safety:
///
/// caller must ensure that disbaled interrupts don't violate any safety guarantees
pub unsafe fn disable_interrupts() {
    asm! {
        "cli"
    }
}

/// # Safety: caller must ensure that interrupts don't violate any safety guarantees
pub unsafe fn enable_interrupts() {
    asm! {
        "sti"
    }
}

/// Get the GS base
#[inline]
pub unsafe fn gs_base() -> u64 {
    IA32_GS_BASE.read()
}

/// Set the GS base
#[inline]
pub unsafe fn set_gs_base(base: u64) {
    IA32_GS_BASE.write(base);
}

/// Get the FS base
#[inline]
pub unsafe fn fs_base() -> u64 {
    IA32_FS_BASE.read()
}

/// Set the FS base
#[inline]
pub unsafe fn set_fs_base(base: u64) {
    IA32_FS_BASE.write(base);
}
