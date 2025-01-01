//! Module that provides the [TaskLocal] data type
//!
//! # Safety:
//!
//! This module assuems that there is no context switching or similar.
//! This entire implementation is unsafe if it is possible to run concurrent code
//! on a single core.
//!
//! TODO reimplement this once I have some sort of job/thread system
//! I would probably want a second function trait like TaskInfo that works similar to CoreInfo and
//! InterruptState

use alloc::{boxed::Box, vec::Vec};
use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
};

use crate::{
    sync::{single_core_lock::SingleCoreLock, CoreInfo, InterruptState},
    types::NotSend,
};

/// Used to initialize a TaskLocal on the first time it is accessed on a core
enum TaskLocalInitializer<T: 'static> {
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
struct TaskLocalInner<T> {
    /// Set to true, after the value is initialized
    initialized: UnsafeCell<bool>,

    /// the value local to the core
    value: UnsafeCell<MaybeUninit<T>>,

    /// Set to true, if the local core value is used by a [TaskLocalRef]
    in_use: UnsafeCell<bool>,

    /// marker to ensure that this is [NotSend]
    _not_send: NotSend,
}

impl<T: 'static> TaskLocalInner<T> {
    #[inline]
    /// ensures that this core local has a value
    fn init(&self, init: &TaskLocalInitializer<T>) {
        // Safety: TaskLocalInner is only ever used on this single core
        // and is protected by [TaskLocalRef]
        let is_init = unsafe { *self.initialized.get() };
        if is_init {
            return;
        }

        let initial_value = match init {
            TaskLocalInitializer::Fn(init) => init(),
            TaskLocalInitializer::BoxFn(init) => init(),
            TaskLocalInitializer::CloneStatic(orig, clone) => clone(*orig),
            TaskLocalInitializer::Clone(orig, clone) => clone(orig),
        };

        // Safety:
        // 1. Ptr read: TaskLocalInner is only ever used on this core and is
        // protected by [TaskLocalRef] so &mut ptr read is ok
        // 2. write (not unsafe): we only ever write once protected by initialized,
        // so we do not skip any drop calls
        unsafe { (*self.value.get()).write(initial_value) };
        // Safety: TaskLocalInner is only ever used on this core
        unsafe { *self.initialized.get() = true };
    }

    /// get a shared reference for this [TaskLocal]
    unsafe fn get(&self, init: &TaskLocalInitializer<T>) -> &T {
        self.init(init);
        unsafe { (*self.value.get()).assume_init_ref() }
    }

    /// get a unique reference for this [TaskLocal]
    unsafe fn get_mut(&self, init: &TaskLocalInitializer<T>) -> &mut T {
        self.init(init);
        unsafe { (*self.value.get()).assume_init_mut() }
    }
}

/// A Core Local Value.
///
/// This allows cores to get access to a [TaskLocalRef] which provides
/// access to the value that is local to the core.
pub struct TaskLocal<T: 'static, I> {
    initializer: TaskLocalInitializer<T>,

    data: Box<[TaskLocalInner<T>]>,

    _core_info: PhantomData<I>,
}

/// A local accessor into a [TaskLocal]
///
/// This works simillar to [core::cell::Ref] ([core::cell::RefCell]), just for the
/// [TaskLocal] access.
pub struct TaskLocalRef<'l, T: 'static, I> {
    container: &'l TaskLocal<T, I>,
    _core_info: PhantomData<I>,
}

impl<T: 'static, I: CoreInfo> TaskLocalRef<'_, T, I> {
    /// Shared access to the [TaskLocal]
    pub fn get(&self) -> &T {
        unsafe {
            // Safety:
            // we can get shared access, because we have shared access on the local ref
            self.container.data[I::s_core_id().0 as usize].get(&self.container.initializer)
        }
    }

    /// Mutable access to the [TaskLocal]
    pub fn get_mut(&mut self) -> &mut T {
        unsafe {
            // Safety:
            // we can get mut access, because we have mut access on the local ref
            self.container.data[I::s_core_id().0 as usize].get_mut(&self.container.initializer)
        }
    }
}

impl<T: Default + 'static, I: InterruptState> Default for TaskLocal<T, I> {
    fn default() -> Self {
        Self::new_with_default()
    }
}

impl<T: Default + 'static, I: InterruptState> TaskLocal<T, I> {
    /// Create a new [TaskLocal] initializing values using the [Default] trait
    pub fn new_with_default() -> Self {
        Self {
            initializer: TaskLocalInitializer::Fn(T::default),
            data: Self::new_empty_data(),
            _core_info: PhantomData,
        }
    }
}

impl<T: 'static, I: InterruptState> TaskLocal<T, I> {
    /// Creates a new [TaskLocal] initializing values using a function ptr
    pub fn new(initializer: fn() -> T) -> Self {
        Self {
            initializer: TaskLocalInitializer::Fn(initializer),
            data: Self::new_empty_data(),
            _core_info: PhantomData,
        }
    }

    /// Creates a new [TaskLocal] initializing values using a boxed function
    pub fn new_with_fn<F>(initializer: F) -> Self
    where
        F: Fn() -> T + 'static,
    {
        Self {
            initializer: TaskLocalInitializer::BoxFn(Box::new(initializer)),
            data: Self::new_empty_data(),
            _core_info: PhantomData,
        }
    }
}

impl<T: Clone + Sync + 'static, I: InterruptState> TaskLocal<T, I> {
    /// Creates a new [TaskLocal] initializing values by cloning
    pub fn new_from_static(original: &'static T) -> Self {
        Self {
            initializer: TaskLocalInitializer::CloneStatic(original, T::clone),
            data: Self::new_empty_data(),
            _core_info: PhantomData,
        }
    }
}

impl<T: Clone + Send + Sync + 'static, I: InterruptState> TaskLocal<T, I> {
    /// Creates a new [TaskLocal] initializing values by cloning
    pub fn new_from_original(original: T) -> Self {
        Self {
            initializer: TaskLocalInitializer::Clone(original, T::clone),
            data: Self::new_empty_data(),
            _core_info: PhantomData,
        }
    }
}

impl<T: 'static, I: InterruptState> TaskLocal<T, I> {
    /// Get a local accessor for the [TaskLocal]
    ///
    /// This returns `None` if an accessor already exists. This is there to prevent
    /// multiple shared references and works similar to [core::cell::RefCell]
    pub fn get(&self) -> Option<TaskLocalRef<'_, T, I>> {
        let local_inner = &self.data[I::s_core_id().0 as usize];
        let in_use_ptr = local_inner.in_use.get();

        let mut core_lock = SingleCoreLock::<I>::new();
        let _lock = core_lock.lock();

        // Safety: TaskLocalInner is only ever used on this core in this function
        // so we currently have mutable access
        if unsafe { *in_use_ptr } {
            return None;
        }

        // Safety: see read above
        unsafe { *in_use_ptr = true }

        return Some(TaskLocalRef {
            container: &self,
            _core_info: PhantomData,
        });
    }

    fn new_empty_data() -> Box<[TaskLocalInner<T>]> {
        let capacity = I::max_core_count() as usize;
        let mut vec = Vec::with_capacity(capacity);

        for _ in 0..capacity {
            vec.push(TaskLocalInner {
                initialized: UnsafeCell::new(false),
                value: UnsafeCell::new(MaybeUninit::uninit()),
                _not_send: NotSend,
                in_use: UnsafeCell::new(false),
            })
        }

        vec.into()
    }
}

impl<T: 'static, I: CoreInfo> AsRef<T> for TaskLocalRef<'_, T, I> {
    fn as_ref(&self) -> &T {
        self.get()
    }
}

impl<T: 'static, I: CoreInfo> AsMut<T> for TaskLocalRef<'_, T, I> {
    fn as_mut(&mut self) -> &mut T {
        self.get_mut()
    }
}

impl<T: 'static, I: CoreInfo> Deref for TaskLocalRef<'_, T, I> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<T: 'static, I: CoreInfo> DerefMut for TaskLocalRef<'_, T, I> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}
