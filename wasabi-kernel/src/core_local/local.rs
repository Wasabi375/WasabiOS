//! Module that provides the [CoreLocal] data type

use alloc::{boxed::Box, vec::Vec};
use core::{
    cell::{RefCell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};
use shared::{
    sync::single_core_lock::SingleCoreLock,
    types::{NotSend, NotSync},
};

use crate::locals;

use super::get_ready_core_count;

enum CoreLocalInitializer<T: 'static> {
    Fn(fn() -> T),
    BoxFn(Box<dyn Fn() -> T>),
    CloneStatic(&'static T, fn(&T) -> T),
    Clone(T, fn(&T) -> T),
}

struct CoreLocalInner<T> {
    initialized: UnsafeCell<bool>,
    value: UnsafeCell<MaybeUninit<T>>,
    _not_send: NotSend,
}

impl<T: 'static> CoreLocalInner<T> {
    #[inline]
    fn init(&self, init: &CoreLocalInitializer<T>) {
        if self.is_init() {
            return;
        }

        let initial_value = match init {
            CoreLocalInitializer::Fn(init) => init(),
            CoreLocalInitializer::BoxFn(init) => init(),
            CoreLocalInitializer::CloneStatic(orig, clone) => clone(*orig),
            CoreLocalInitializer::Clone(orig, clone) => clone(orig),
        };

        let _core_lock = SingleCoreLock::new().lock();

        if self.is_init() {
            // There could have been a context switch after the first check and now.
            // Now that we have a lock we do not need to check again
            return;
        }

        // Safety:
        // 1. Ptr read: CoreLocalInner is only ever used on this core and we have
        // locked the core from context switching so &mut ptr read is ok
        // 2. write (not unsafe): we only ever write once protected by initialized,
        // so we do not skip any drop calls
        unsafe { (*self.value.get()).write(initial_value) };
        // Safety: CoreLocalInner is only ever used on this core and we have locked
        // the core from context switching
        unsafe { *self.initialized.get() = true };
    }

    #[inline]
    fn is_init(&self) -> bool {
        // Safety: CoreLocalInner is only ever used on this single core
        unsafe { *self.initialized.get() }
    }

    unsafe fn get(&self, init: &CoreLocalInitializer<T>) -> &T {
        self.init(init);
        unsafe { (*self.value.get()).assume_init_ref() }
    }

    unsafe fn get_mut(&self, init: &CoreLocalInitializer<T>) -> &mut T {
        self.init(init);
        unsafe { (*self.value.get()).assume_init_mut() }
    }
}

pub struct CoreLocalKey<'l, T: 'static> {
    container: &'l CoreLocal<T>,
    _not_send: NotSend,
    _not_sync: NotSync,
}

impl<T: 'static> CoreLocalKey<'_, T> {
    pub fn get(&self) -> &T {
        unsafe {
            // Safety:
            // we can get shared access, because we have shared access on the local key
            self.container.data[locals!().core_id.0 as usize].get(&self.container.initializer)
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe {
            // Safety:
            // we can get mut access, because we have mut access on the local key
            self.container.data[locals!().core_id.0 as usize].get_mut(&self.container.initializer)
        }
    }
}

pub struct CoreLocal<T: 'static> {
    initializer: CoreLocalInitializer<T>,

    data: Box<[CoreLocalInner<T>]>,

    keys: Box<[AtomicBool]>,
}

impl<T: Default + 'static> Default for CoreLocal<T> {
    fn default() -> Self {
        Self::new_with_default()
    }
}

impl<T: Default + 'static> CoreLocal<T> {
    pub fn new_with_default() -> Self {
        Self {
            initializer: CoreLocalInitializer::Fn(T::default),
            data: Self::new_empty_data(),
        }
    }
}

impl<T: 'static> CoreLocal<T> {
    pub fn new(initializer: fn() -> T) -> Self {
        Self {
            initializer: CoreLocalInitializer::Fn(initializer),
            data: Self::new_empty_data(),
        }
    }

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
    pub fn new_from_static(original: &'static T) -> Self {
        Self {
            initializer: CoreLocalInitializer::CloneStatic(original, T::clone),
            data: Self::new_empty_data(),
        }
    }
}

impl<T: Clone + Send + Sync + 'static> CoreLocal<T> {
    pub fn new_from_original(original: T) -> Self {
        Self {
            initializer: CoreLocalInitializer::Clone(original, T::clone),
            data: Self::new_empty_data(),
        }
    }
}

impl<T: 'static> CoreLocal<T> {
    #[inline]
    fn assert_valid_access() {
        assert!(!locals!().in_interrupt());
        assert!(!locals!().in_exception());
        assert!(locals!().initialized);
    }

    fn new_empty_data() -> Box<[CoreLocalInner<T>]> {
        let capacity = get_ready_core_count(Ordering::Acquire) as usize;
        let mut vec = Vec::with_capacity(capacity);

        for _ in 0..capacity {
            vec.push(CoreLocalInner {
                initialized: UnsafeCell::new(false),
                value: UnsafeCell::new(MaybeUninit::uninit()),
                _not_send: PhantomData,
            })
        }

        vec.into()
    }
}

impl<T: 'static> AsRef<T> for CoreLocalKey<'_, T> {
    fn as_ref(&self) -> &T {
        self.get()
    }
}

impl<T: 'static> AsMut<T> for CoreLocalKey<'_, T> {
    fn as_mut(&mut self) -> &mut T {
        self.get_mut()
    }
}

impl<T: 'static> Deref for CoreLocalKey<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T: 'static> DerefMut for CoreLocalKey<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}
