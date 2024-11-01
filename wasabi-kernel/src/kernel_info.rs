//! Module containing basic information about the kernel
//!
//! This data is either constant or written by the bsp during it's startup.
//!
//! It is not valid for any AP processor to read or write any data here before
//! [KERNEL_MAIN] is reached.
//!
//! Before [KERNEL_MAIN] is reached the BSP may read any write any data.
//! The BSP is not allowed to modify this from an interrupt.
//! After [KERNEL_MAIN] the BSP is only allowed to read.
//!
//! [KERNEL_MAIN]: super::enter_kernel_main

use core::{mem::MaybeUninit, ptr::addr_of_mut};

use bootloader_api::BootInfo;

use crate::{in_kernel_main, locals};

static mut KERNEL_INFO: MaybeUninit<KernelInfo> = MaybeUninit::uninit();

/// Static information about the kernel.
///
/// This includes the [BootInfo] for the bootloader
pub struct KernelInfo {
    /// The bootloader [BootInfo]
    pub boot_info: &'static mut BootInfo,
    /// The index into the page table that contains the recursive page table page
    pub recursive_index: u16,
}

impl KernelInfo {
    /// Get read access to the [KernelInfo].
    ///
    /// This should only be called on the bsp or on an AP after kernel main.
    pub fn get<'a>() -> &'a Self {
        assert!(locals!().is_bsp() || in_kernel_main());

        // Safety: Kernel ensures that [Self::init] is the first function
        // ever called.
        unsafe { (&mut *addr_of_mut!(KERNEL_INFO)).assume_init_ref() }
    }

    /// Get write access to the [KernelInfo].
    ///
    /// This is only possible on the bsp during startup before kernel main.
    pub fn get_mut_for_bsp<'a>() -> &'a mut Self {
        let locals = locals!();
        assert!(
            locals.is_bsp()
                && !in_kernel_main()
                && !locals.in_interrupt()
                && !locals.in_exception()
        );

        // Safety:
        // 1. Kernel ensures that [Self::init] is the first function
        //      ever called.
        // 2. We are before kernel_main outside of interrupt and therefor have unique access
        unsafe { (&mut *addr_of_mut!(KERNEL_INFO)).assume_init_mut() }
    }

    /// Initializes the [KernelInfo] data store
    ///
    /// # Safety
    ///
    /// Must be the first function ever called on bsp and may only be called once
    pub(super) unsafe fn init(boot_info: &'static mut BootInfo) {
        let recursive_index = *boot_info
            .recursive_index
            .as_ref()
            .expect("Expected boot info to contain recursive index");
        unsafe {
            // Safety: on bsp before anything else could even try to access this
            (&mut *addr_of_mut!(KERNEL_INFO)).write(KernelInfo {
                boot_info,
                recursive_index,
            });
        }
    }
}
