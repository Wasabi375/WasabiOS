//! Kernel specific utilities to support the [crossbeam-epoch]
//! package
//!
//! This includes functions like [pin], [default_collector] and [is_pinned]
//!
//!
//! [crossbeam-epoch]: https://docs.rs/crossbeam-epoch/latest/crossbeam_epoch/index.html

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ptr::{addr_of, addr_of_mut},
};

use crossbeam_epoch::{Collector, Guard, LocalHandle};

use crate::locals;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

static mut COLLECTOR: MaybeUninit<Collector> = MaybeUninit::uninit();

/// Access the core local [LocalHandle]
///
/// # Panics
///
/// This panics if this is called from an interrupt or exception.
pub fn local_handle() -> &'static LocalHandle {
    assert!(!locals!().in_interrupt());
    assert!(!locals!().in_exception());

    unsafe {
        // Safety: only ever written in [processor_init] which is only ever called once,
        // before this function is called.
        let handle = &*locals!().epoch_handle.handle.get();

        // Safety: This is save under the assumption that [bsp_init] and [processor_init] have been
        // called
        handle.assume_init_ref()
    }
}

/// Pins the current core
///
/// # Panics
///
/// This panics if this is called from an interrupt or exception
pub fn pin() -> Guard {
    local_handle().pin()
}

/// Returns `true` if the current core is pinned.
///
/// # Panics
///
/// This panics if this is called from an interrupt or exception
pub fn is_pinned() -> bool {
    local_handle().is_pinned()
}

/// Returns the default global collector.
///
/// # Panics
///
/// This panics if this is called from an interrupt or exception
pub fn default_collector() -> &'static Collector {
    unsafe {
        // Safety: reference only exist as a shared ref, while mut refs only exist
        // during [bsp_init] which can no longer be called
        let collector = &*addr_of!(COLLECTOR);
        // Safety: This is save under the assumption that [bsp_init] has been called
        collector.assume_init_ref()
    }
}

/// Initializes Kernel statics for the crossbeam-epoch package
///
/// # Safety
///
/// must only ever be called once during kernel startup after memory is initialized
pub unsafe fn bsp_init() {
    let collector = unsafe {
        // Safety: mut ref only last for this function and this function can not be
        // called concurrently with any shared access
        &mut *addr_of_mut!(COLLECTOR)
    };
    collector.write(Collector::new());
    info!("crossbeam-epoch Collector initialized");
}

/// Initializes core local statics for the crossbeam-epoch package
///
/// # Safety
///
/// must be called once per processor during startup and only after [bsp_init] has finished
/// and with core locals initialized.
pub unsafe fn processor_init() {
    let local_handle = default_collector().register();
    let core_local_handle = unsafe {
        // Safety: only ever written during processor_init which is only executed once per core
        // and this is a core local variable
        &mut *locals!().epoch_handle.handle.get()
    };
    core_local_handle.write(local_handle);
    info!("crossbeam-epoch LocalHandle initialized");
}

/// A wrapper for the [LocalHandle] stored within [crate::core_local::CoreStatics].
///
/// The [LocalHandle] can be accessed using [local_handle].
pub struct LocalEpochHandle {
    /// The core local handle
    handle: UnsafeCell<MaybeUninit<LocalHandle>>,
}

impl LocalEpochHandle {
    /// creates an uninitialized [LocalEpochHandle].
    ///
    /// This should only be stored within [crate::core_local::CoreStatics]
    /// and is initialized by [processor_init].
    pub const fn new_uninit() -> Self {
        LocalEpochHandle {
            handle: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }
}

#[cfg(feature = "test")]
mod test {
    use testing::{kernel_test, t_assert, KernelTestError};

    use super::*;

    #[kernel_test]
    fn test_default_not_pinned() -> Result<(), KernelTestError> {
        t_assert!(!is_pinned());
        Ok(())
    }

    #[kernel_test(allow_heap_leak)] // TODO memory leak
    fn test_register_local_handle() -> Result<(), KernelTestError> {
        let collector = default_collector();

        let handle = collector.register();
        t_assert!(!handle.is_pinned());

        let guard = handle.pin();
        t_assert!(handle.is_pinned());

        drop(guard);
        t_assert!(!handle.is_pinned());
        Ok(())
    }

    #[kernel_test]
    fn test_pin() -> Result<(), KernelTestError> {
        t_assert!(!is_pinned());

        let guard = pin();
        t_assert!(is_pinned());

        let guard_2 = pin();
        t_assert!(is_pinned());

        drop(guard);
        t_assert!(is_pinned());

        drop(guard_2);
        t_assert!(!is_pinned());
        Ok(())
    }

    #[kernel_test]
    fn test_core_local_handle() -> Result<(), KernelTestError> {
        let handle = local_handle();
        t_assert!(!is_pinned());
        t_assert!(!handle.is_pinned());

        let guard = handle.pin();
        t_assert!(is_pinned());
        t_assert!(handle.is_pinned());

        let guard_2 = pin();
        t_assert!(is_pinned());
        t_assert!(handle.is_pinned());

        drop(guard_2);
        t_assert!(is_pinned());
        t_assert!(handle.is_pinned());

        drop(guard);
        t_assert!(!is_pinned());
        t_assert!(!handle.is_pinned());

        Ok(())
    }
}
