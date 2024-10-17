//! Ptr types provided by the kernel

use core::{
    fmt::{Debug, Pointer},
    ptr::NonNull,
};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};
use volatile::{access::ReadOnly, Volatile};
use x86_64::VirtAddr;

/// A ptr to an untyped region of memory.
///
/// # Safety
///
/// The memory must be mapped exactly once in the context this is used in.
/// It is valid to map multiple times in a different context and it is therefor
/// not safe to assume that this memory can be accessed without breaking rust
/// mutability guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UntypedPtr(NonNull<u8>);

impl UntypedPtr {
    /// Constructs a new [UntypedPtr]
    ///
    /// # Safety:
    /// The caller must guarantee that `ptr` is mapped in the current context
    pub unsafe fn new(vaddr: VirtAddr) -> Option<Self> {
        NonNull::new(vaddr.as_mut_ptr()).map(|ptr| ptr.into())
    }
    /// Constructs a new [UntypedPtr]
    ///
    /// # Safety:
    /// The caller must guarantee that `ptr` is mapped in the current context
    pub unsafe fn new_from_raw(ptr: *mut u8) -> Option<Self> {
        NonNull::new(ptr).map(|ptr| ptr.into())
    }

    /// Provides access to the underlying [NonNull]
    pub fn into_inner(self) -> NonNull<u8> {
        self.0
    }

    /// Calculates the offset for the ptr in bytes
    ///
    /// # Safety
    ///
    /// This relies on [NonNull::offset] and follows the same safety requirments
    pub unsafe fn offset(self, bytes: isize) -> UntypedPtr {
        unsafe { UntypedPtr(self.0.offset(bytes)) }
    }

    /// Calculates the offset for the ptr in bytes
    ///
    /// This is the same as `ptr.offset(bytes as isize)`.
    /// See [NonNull::add] for why the cast to signed must be safe.
    ///
    /// # Safety
    ///
    /// This relies on [NonNull::add] and follows the same safety requirments
    pub unsafe fn add(self, bytes: usize) -> UntypedPtr {
        unsafe { UntypedPtr(self.0.add(bytes)) }
    }

    /// Casts the pointer into a [NonNull] of type `T`
    pub fn cast<T>(self) -> NonNull<T> {
        self.0.cast()
    }

    /// Casts the pinter into a [NonNull] of type `[T]`
    pub fn cast_slice<T>(self, len: usize) -> NonNull<[T]> {
        NonNull::slice_from_raw_parts(self.cast(), len)
    }

    /// Converts the pointer into a raw pointer
    pub fn as_ptr<T>(self) -> *mut T {
        self.0.as_ptr().cast()
    }

    /// Casts the pointer to a reference of type `T`
    ///
    /// # Safety
    ///
    /// The caller must guarantee the following:
    ///  1. This points to a valid object of type `T`.
    ///  2. This points to memory that is not currently borrowed mutably
    pub unsafe fn as_ref<'a, T>(self) -> &'a T {
        unsafe { &*self.as_ptr() }
    }

    /// Casts the pointer to a mutable reference of type `T`
    ///
    /// # Safety
    ///
    /// The caller must guarantee the following:
    ///  1. This points to a valid object of type `T`.
    ///  2. This points to memory that is not currently borrowed
    pub unsafe fn as_mut<'a, T>(self) -> &'a mut T {
        unsafe { &mut *self.as_ptr() }
    }

    /// returns a [Volatile] that provides access to this ptr
    ///
    /// Safety: Pointer must be a valid *mut* reference
    pub unsafe fn as_volatile<'a, T>(self) -> Volatile<&'a T, ReadOnly> {
        unsafe { Volatile::new_read_only(self.as_ref()) }
    }

    /// returns a [Volatile] that provides access to this ptr
    ///
    /// Safety: Pointer must be a valid *mut* reference
    pub unsafe fn as_volatile_mut<'a, T>(self) -> Volatile<&'a mut T> {
        unsafe { Volatile::new(self.as_mut()) }
    }

    /// Calls `memset`.
    ///
    /// See [NonNull::write_bytes].
    ///
    /// # Saftey
    ///
    /// The caller must guarantee the following:
    ///  1. This points to a valid memory for the next `count` bytes.
    ///  2. This points to memory that is not currently borrowed
    pub unsafe fn write_bytes(self, val: u8, count: usize) {
        unsafe { self.0.write_bytes(val, count) }
    }
}

impl From<NonNull<u8>> for UntypedPtr {
    fn from(value: NonNull<u8>) -> Self {
        Self(value)
    }
}

impl Pointer for UntypedPtr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Pointer::fmt(&self.0, f)
    }
}
