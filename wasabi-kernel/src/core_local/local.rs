//! Module that provides the [CoreLocal] data type
//!
//! # Safety:
//!
//! This module assuems that there is no context switching or similar.
//! This entire implementation is unsafe if it is possible to run concurrent code
//! on a single core.
//!
//! TODO reimplement this once I have some sort of job/thread system

use alloc::{boxed::Box, vec::Vec};
use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    sync::atomic::Ordering,
};
use shared::types::NotSend;

use crate::{locals, prelude::SingleCoreLock};

use super::get_ready_core_count;

/// Used to initialize a CoreLocal on the first time it is accessed on a core
enum CoreLocalInitializer<T: 'static> {
    /// Initialize via function call
    Fn(fn() -> T),
    /// Initialize via boxed function (allows for lambda)
    BoxFn(Box<dyn Fn() -> T>),
    /// Initialize via cloning a value from a static reference
    CloneStatic(&'static T, fn(&T) -> T),
    /// Initialize via cloning from an owned value
    Clone(T, fn(&T) -> T),
}

/// The value stored on a given core
struct CoreLocalInner<T> {
    /// Set to true, after the value is initialized
    initialized: UnsafeCell<bool>,

    /// the value local to the core
    value: UnsafeCell<MaybeUninit<T>>,

    /// Set to true, if the local core value is used by a [CoreLocalRef]
    in_use: UnsafeCell<bool>,

    /// marker to ensure that this is [NotSend]
    _not_send: NotSend,
}

impl<T: 'static> CoreLocalInner<T> {
    #[inline]
    /// ensures that this core local has a value
    fn init(&self, init: &CoreLocalInitializer<T>) {
        // Safety: CoreLocalInner is only ever used on this single core
        // and is protected by [CoreLocalRef]
        let is_init = unsafe { *self.initialized.get() };
        if is_init {
            return;
        }

        let initial_value = match init {
            CoreLocalInitializer::Fn(init) => init(),
            CoreLocalInitializer::BoxFn(init) => init(),
            CoreLocalInitializer::CloneStatic(orig, clone) => clone(*orig),
            CoreLocalInitializer::Clone(orig, clone) => clone(orig),
        };

        // Safety:
        // 1. Ptr read: CoreLocalInner is only ever used on this core and is
        // protected by [CoreLocalRef] so &mut ptr read is ok
        // 2. write (not unsafe): we only ever write once protected by initialized,
        // so we do not skip any drop calls
        unsafe { (*self.value.get()).write(initial_value) };
        // Safety: CoreLocalInner is only ever used on this core
        unsafe { *self.initialized.get() = true };
    }

    /// get a shared reference for this [CoreLocal]
    unsafe fn get(&self, init: &CoreLocalInitializer<T>) -> &T {
        self.init(init);
        unsafe { (*self.value.get()).assume_init_ref() }
    }

    /// get a unique reference for this [CoreLocal]
    unsafe fn get_mut(&self, init: &CoreLocalInitializer<T>) -> &mut T {
        self.init(init);
        unsafe { (*self.value.get()).assume_init_mut() }
    }
}

/// A Core Local Value.
///
/// This allows cores to get access to a [CoreLocalRef] which provides
/// access to the value that is local to the core.
pub struct CoreLocal<T: 'static> {
    initializer: CoreLocalInitializer<T>,

    data: Box<[CoreLocalInner<T>]>,
}

pub struct CoreLocalRef<'l, T: 'static> {
    container: &'l CoreLocal<T>,
}

impl<T: 'static> CoreLocalRef<'_, T> {
    /// Shared access to the [CoreLocal]
    pub fn get(&self) -> &T {
        unsafe {
            // Safety:
            // we can get shared access, because we have shared access on the local ref
            self.container.data[locals!().core_id.0 as usize].get(&self.container.initializer)
        }
    }

    /// Mutable access to the [CoreLocal]
    pub fn get_mut(&mut self) -> &mut T {
        unsafe {
            // Safety:
            // we can get mut access, because we have mut access on the local ref
            self.container.data[locals!().core_id.0 as usize].get_mut(&self.container.initializer)
        }
    }
}

impl<T: Default + 'static> Default for CoreLocal<T> {
    fn default() -> Self {
        Self::new_with_default()
    }
}

impl<T: Default + 'static> CoreLocal<T> {
    /// Create a new [CoreLocal] initializing values using the [Default] trait
    pub fn new_with_default() -> Self {
        Self {
            initializer: CoreLocalInitializer::Fn(T::default),
            data: Self::new_empty_data(),
        }
    }
}

impl<T: 'static> CoreLocal<T> {
    /// Creates a new [CoreLocal] initializing values using a function ptr
    pub fn new(initializer: fn() -> T) -> Self {
        Self {
            initializer: CoreLocalInitializer::Fn(initializer),
            data: Self::new_empty_data(),
        }
    }

    /// Creates a new [CoreLocal] initializing values using a boxed function
    pub fn new_with_fn<F>(initializer: F) -> Self
    where
        F: Fn() -> T + 'static,
    {
        Self {
            initializer: CoreLocalInitializer::BoxFn(Box::new(initializer)),
            data: Self::new_empty_data(),
        }
    }
}

impl<T: Clone + Sync + 'static> CoreLocal<T> {
    /// Creates a new [CoreLocal] initializing values by cloning
    pub fn new_from_static(original: &'static T) -> Self {
        Self {
            initializer: CoreLocalInitializer::CloneStatic(original, T::clone),
            data: Self::new_empty_data(),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> CoreLocal<T> {
    /// Creates a new [CoreLocal] initializing values by cloning
    pub fn new_from_original(original: T) -> Self {
        Self {
            initializer: CoreLocalInitializer::Clone(original, T::clone),
            data: Self::new_empty_data(),
        }
    }
}

impl<T: 'static> CoreLocal<T> {
    /// Get a local accessor for the [CoreLocal]
    ///
    /// This returns `None` if an accessor already exists. This is there to prevent
    /// multiple shared references and works similar to [core::cell::RefCell]
    pub fn get(&self) -> Option<CoreLocalRef<'_, T>> {
        let local_inner = &self.data[locals!().core_id.0 as usize];
        let in_use_ptr = local_inner.in_use.get();

        let mut core_lock = SingleCoreLock::new();
        let _lock = core_lock.lock();

        // Safety: CoreLocalInner is only ever used on this core in this function
        // so we currently have mutable access
        if unsafe { *in_use_ptr } {
            return None;
        }

        // Safety: see read above
        unsafe { *in_use_ptr = true }

        return Some(CoreLocalRef { container: &self });
    }

    fn new_empty_data() -> Box<[CoreLocalInner<T>]> {
        let capacity = get_ready_core_count(Ordering::Acquire) as usize;
        let mut vec = Vec::with_capacity(capacity);

        for _ in 0..capacity {
            vec.push(CoreLocalInner {
                initialized: UnsafeCell::new(false),
                value: UnsafeCell::new(MaybeUninit::uninit()),
                _not_send: NotSend,
                in_use: UnsafeCell::new(false),
            })
        }

        vec.into()
    }
}

impl<T: 'static> AsRef<T> for CoreLocalRef<'_, T> {
    fn as_ref(&self) -> &T {
        self.get()
    }
}

impl<T: 'static> AsMut<T> for CoreLocalRef<'_, T> {
    fn as_mut(&mut self) -> &mut T {
        self.get_mut()
    }
}

impl<T: 'static> Deref for CoreLocalRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T: 'static> DerefMut for CoreLocalRef<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}
