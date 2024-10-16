//! A module containing cpu utilities as well as more specific sub modules.

pub mod acpi;
pub mod apic;
pub mod cpuid;
pub mod gdt;
pub mod interrupts;

pub use instructions::*;

#[allow(unsafe_op_in_unsafe_fn)]
mod instructions {
    use core::ptr::addr_of_mut;

    use x86_64::{
        instructions::{self, interrupts},
        registers::model_specific::Msr,
    };

    /// MSR for active FS base
    static mut IA32_FS_BASE: Msr = Msr::new(0xc0000100);

    unsafe fn ia32_fs_base() -> &'static mut Msr {
        &mut *addr_of_mut!(IA32_FS_BASE)
    }

    /// MSR for active GS base
    static mut IA32_GS_BASE: Msr = Msr::new(0xc0000101);

    unsafe fn ia32_gs_base() -> &'static mut Msr {
        &mut *addr_of_mut!(IA32_GS_BASE)
    }

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

    /// Disbales interrupts.
    ///
    /// When possible `locals!().disbale_interrupts()` should be used instead.
    ///
    /// ## See:
    /// [crate::core_local::CoreLocals]  
    /// [crate::locals]
    ///
    /// # Safety:
    ///
    /// caller must ensure that disbaled interrupts don't violate any safety guarantees
    #[inline(always)]
    pub unsafe fn disable_interrupts() {
        interrupts::disable();
    }

    /// # Safety: caller must ensure that interrupts don't violate any safety guarantees
    ///
    /// When possible `locals!().disbale_interrupts()` should be used instead.
    ///
    /// ## See:
    /// [crate::core_local::CoreLocals]  
    /// [crate::locals]
    #[inline(always)]
    pub unsafe fn enable_interrupts() {
        interrupts::enable();
    }

    /// Get the GS base
    #[inline]
    pub fn gs_base() -> u64 {
        // Safety: accessing reading gs segment is save
        unsafe { ia32_gs_base().read() }
    }

    /// Set the GS base
    #[inline]
    pub fn set_gs_base(base: u64) {
        unsafe {
            // Safety: accessing reading gs segment is save
            ia32_gs_base().write(base);
        }
    }

    /// Get the FS base
    #[inline]
    pub fn fs_base() -> u64 {
        // Safety: accessing reading fs segment is save
        unsafe { ia32_fs_base().read() }
    }

    /// Set the FS base
    #[inline]
    pub fn set_fs_base(base: u64) {
        unsafe {
            // Safety: accessing reading fs segment is save
            ia32_fs_base().write(base);
        }
    }
}
