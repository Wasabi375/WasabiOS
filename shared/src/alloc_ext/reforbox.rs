//! Contains [RefOrBox] an enum that allows implementations of features
//! to work equaly witha Box or just a reference to the underlying type
use core::{borrow::Borrow, ops::Deref};

use alloc::boxed::Box;

/// Can be either a reference to `T` or an owned value of type `O` that derefs to `T`
///
/// # Example
/// `RefOrOwn<str, Box<str>>`
// TODO can't I just use Cow here?
pub enum RefOrBox<'a, T: ?Sized + 'a> {
    /// value is accessed via reference
    Ref(&'a T),
    /// value is owned and accessed directly
    Boxed(Box<T>),
}

impl<T> Deref for RefOrBox<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            RefOrBox::Ref(value) => value,
            RefOrBox::Boxed(value) => value.borrow(),
        }
    }
}

impl<T> RefOrBox<'_, T> {
    /// Returns the owned value, if it exists
    ///
    /// otherwise returns self with the original reference
    pub fn into_owned(self) -> Result<T, Self> {
        match self {
            RefOrBox::Ref(_) => Err(self),
            RefOrBox::Boxed(value) => Ok(Box::into_inner(value)),
        }
    }
}

impl<T> RefOrBox<'_, T> {
    /// Makes a [RefOrOwn] that owns the value
    ///
    /// thereby extending the lifetime to 'static.
    pub fn make_owned(self) -> RefOrBox<'static, T>
    where
        T: Clone,
    {
        match self {
            RefOrBox::Ref(value) => RefOrBox::Boxed(Box::from(value.clone())),
            RefOrBox::Boxed(value) => RefOrBox::Boxed(value),
        }
    }
}
