use core::borrow::{Borrow, BorrowMut};
use core::cmp::Ordering;
use core::fmt::{self, Debug, Formatter};
use core::hash::{Hash, Hasher};
use core::mem::MaybeUninit;
use core::ops::{
    Deref, DerefMut, Index, IndexMut, Range, RangeFrom, RangeFull, RangeInclusive, RangeTo,
    RangeToInclusive,
};
use core::ptr;
use core::slice::{from_raw_parts, from_raw_parts_mut};

use crate::heap::StaticHeap;
use crate::iterators::{StaticVecIntoIter, StaticVecIterConst, StaticVecIterMut};
use crate::string::StaticString;
use crate::utils::partial_compare;
use crate::StaticVec;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

#[cfg(feature = "serde")]
use core::marker::PhantomData;

#[cfg(feature = "serde")]
use serde::{
    de::{SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize, Serializer,
};
use shared::math::Number;

// Note that the constness of many of the trait impls in this file varies in how useful it actually
// is currently, and is done mostly just out of the desire to be able to quickly "stay on top" of
// future developments as far as `const_trait_impl` is concerned.

impl<T, L, const N: usize> AsMut<[T]> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn as_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, L, const N: usize> AsRef<[T]> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, L, const N: usize> Borrow<[T]> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn borrow(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, L, const N: usize> BorrowMut<[T]> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn borrow_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: Clone, L, const N: usize> Clone for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline]
    default fn clone(&self) -> Self {
        let mut res = Self::new();
        for item in self {
            // Safety: `self` has the same capacity of `res`, and `res` is
            // empty, so all of these pushes are safe.
            unsafe {
                res.push_unchecked(item.clone());
            }
        }
        res
    }

    #[inline]
    default fn clone_from(&mut self, other: &Self) {
        let other_length = other.length;
        self.truncate(other_length);
        let self_length = self.length.try_into().unwrap();
        for i in 0..self_length {
            let i = i.try_into().unwrap();
            // Safety: after the truncate, `self.len` <= `other.len`, which means that for
            // every `i` in `self`, there is definitely an element at `other[i]`.
            unsafe {
                self.get_unchecked_mut(i).clone_from(other.get_unchecked(i));
            }
        }
        for i in self_length..other_length.try_into().unwrap() {
            let i = i.try_into().unwrap();
            // Safety: `i` < `other.length`, so `other.get_unchecked` is safe. `i` starts at
            // `self.length`, which is <= `other.length`, so there is always an available
            // slot at `self[i]` to push into.
            unsafe {
                self.push_unchecked(other.get_unchecked(i).clone());
            }
        }
    }
}

impl<T: Copy, L, const N: usize> Clone for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn clone(&self) -> Self {
        let length = self.length.try_into().unwrap();
        match length {
            // If `self` is empty, just return a new StaticVec.
            0 => Self::new(),
            _ => Self {
                data: {
                    let mut res = StaticVec::new_data_uninit();
                    unsafe {
                        self.as_ptr()
                            .copy_to_nonoverlapping(StaticVec::first_ptr_mut(&mut res), length);
                        res
                    }
                },
                length: length.try_into().unwrap(),
            },
        }
    }

    #[inline(always)]
    fn clone_from(&mut self, rhs: &Self) {
        // Similar to what we do above, but more straightforward since `clone_from` works in-place.
        unsafe {
            self.as_mut_ptr()
                .copy_from_nonoverlapping(rhs.as_ptr(), rhs.length.try_into().unwrap());
            self.set_len(rhs.length);
        }
    }
}

impl<T: Debug, L, const N: usize> Debug for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.debug_list().entries(self.as_slice()).finish()
    }
}

impl<T, L, const N: usize> Default for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Calls `new`.
    #[inline(always)]
    fn default() -> Self {
        Self::new()
    }
}

impl<T, L, const N: usize> Deref for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Target = [T];
    #[inline(always)]
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, L, const N: usize> DerefMut for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, L, const N: usize> Drop for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn drop(&mut self) {
        // `self.as_mut_slice()` will always return a slice of known-initialized elements.
        unsafe { ptr::drop_in_place(self.as_mut_slice()) };
    }
}

impl<T, L, const N: usize> Eq for StaticVec<T, N, L>
where
    T: Eq,
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
}

/// A helper trait for specialization-based implementations of [`Extend`](core::iter::Extend) and
/// ['FromIterator`](core::iter::FromIterator).
pub(crate) trait ExtendEx<T, I> {
    fn extend_ex(&mut self, iter: I);
    fn from_iter_ex(iter: I) -> Self;
}

impl<T, I: IntoIterator<Item = T>, const N: usize, L> ExtendEx<T, I> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Appends all elements, if any, from `iter` to the StaticVec. If `iter` has a size greater
    /// than the StaticVec's capacity, any items after that point are ignored.
    #[inline]
    default fn extend_ex(&mut self, iter: I) {
        let mut it = iter.into_iter();
        let mut i = self.length;
        while i < N.try_into().unwrap() {
            if let Some(val) = it.next() {
                unsafe {
                    self.mut_ptr_at_unchecked(i).write(val);
                }
            } else {
                break;
            }
            i += L::ONE;
        }
        self.length = i;
    }

    /// Creates a new StaticVec instance from the elements, if any, of `iter`.
    /// If `iter` has a size greater than the StaticVec's capacity, any items after
    /// that point are ignored.
    #[inline]
    default fn from_iter_ex(iter: I) -> Self {
        let mut res = StaticVec::new_data_uninit();
        let mut it = iter.into_iter();
        let mut i = 0;
        while i < N {
            if let Some(val) = it.next() {
                unsafe {
                    StaticVec::<T, N>::first_ptr_mut(&mut res).add(i).write(val);
                }
            } else {
                break;
            }
            i += 1;
        }
        Self {
            data: res,
            length: i.try_into().unwrap(),
        }
    }
}

impl<'a, T: 'a + Copy, I: IntoIterator<Item = &'a T>, const N: usize, L> ExtendEx<&'a T, I>
    for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Appends all elements, if any, from `iter` to the StaticVec. If `iter` has a size greater
    /// than the StaticVec's capacity, any items after that point are ignored.
    #[inline]
    default fn extend_ex(&mut self, iter: I) {
        let mut it = iter.into_iter();
        let mut i = self.length;
        while i < N.try_into().unwrap() {
            if let Some(val) = it.next() {
                unsafe {
                    self.mut_ptr_at_unchecked(i).write(*val);
                }
            } else {
                break;
            }
            i += L::ONE;
        }
        self.length = i;
    }

    /// Creates a new StaticVec instance from the elements, if any, of `iter`.
    /// If `iter` has a size greater than the StaticVec's capacity, any items after
    /// that point are ignored.
    #[inline]
    default fn from_iter_ex(iter: I) -> Self {
        let mut res = StaticVec::new_data_uninit();
        let mut it = iter.into_iter();
        let mut i = 0;
        while i < N {
            if let Some(val) = it.next() {
                unsafe {
                    StaticVec::<T, N>::first_ptr_mut(&mut res)
                        .add(i)
                        .write(*val);
                }
            } else {
                break;
            }
            i += 1;
        }
        Self {
            data: res,
            length: i.try_into().unwrap(),
        }
    }
}

impl<T, const N1: usize, const N2: usize, L1, L2> ExtendEx<T, StaticVec<T, N1, L1>>
    for StaticVec<T, N2, L2>
where
    L1: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L1 as TryFrom<usize>>::Error: Debug,
    <L1 as TryInto<usize>>::Error: Debug,
    L2: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L2 as TryFrom<usize>>::Error: Debug,
    <L2 as TryInto<usize>>::Error: Debug,
{
    #[inline]
    default fn extend_ex(&mut self, iter: StaticVec<T, N1, L1>) {
        // We just reuse most of the `extend_from_slice` code here.
        let old_length = self.length;
        let added_length: usize = iter
            .len()
            .try_into()
            .unwrap()
            .min(N2 - old_length.try_into().unwrap());
        // Safety: added_length is <= our remaining capacity and `iter.len()`.
        unsafe {
            iter.as_ptr()
                .copy_to_nonoverlapping(self.mut_ptr_at_unchecked(old_length), added_length);
            self.set_len(old_length + added_length.try_into().unwrap());

            let drop_range = N1.min(N2)..iter.len().try_into().unwrap();
            if !drop_range.is_empty() {
                // Wrap the values in a MaybeUninit to inhibit their destructors (if any),
                // then manually drop any excess ones. This is the same kind of "trick" as
                // is used in `new_from_array`, as you may or may not have noticed.
                let mut forgotten = MaybeUninit::new(iter);

                ptr::drop_in_place(
                    forgotten
                        .assume_init_mut()
                        .as_mut_slice()
                        .get_unchecked_mut(drop_range),
                );
            }
        }
    }

    #[inline]
    default fn from_iter_ex(iter: StaticVec<T, N1, L1>) -> Self {
        Self {
            data: {
                unsafe {
                    let mut data = StaticVec::new_data_uninit();
                    iter.as_ptr()
                        .copy_to_nonoverlapping(StaticVec::first_ptr_mut(&mut data), N1.min(N2));
                    // Same thing as above here.
                    let mut forgotten = MaybeUninit::new(iter);
                    ptr::drop_in_place(
                        forgotten
                            .assume_init_mut()
                            .as_mut_slice()
                            .get_unchecked_mut(N1.min(N2)..N1),
                    );
                    data
                }
            },
            length: N1.min(N2).try_into().unwrap(),
        }
    }
}

impl<'a, T: 'a + Copy, const N1: usize, const N2: usize, L1, L2>
    ExtendEx<&'a T, &StaticVec<T, N2, L2>> for StaticVec<T, N1, L1>
where
    L1: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L1 as TryFrom<usize>>::Error: Debug,
    <L1 as TryInto<usize>>::Error: Debug,
    L2: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L2 as TryFrom<usize>>::Error: Debug,
    <L2 as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    default fn extend_ex(&mut self, iter: &StaticVec<T, N2, L2>) {
        self.extend_from_slice(iter);
    }

    #[inline(always)]
    default fn from_iter_ex(iter: &StaticVec<T, N2, L2>) -> Self {
        Self::new_from_slice(iter)
    }
}

impl<'a, T: 'a + Copy, const N: usize, L> ExtendEx<&'a T, &StaticVec<T, N, L>>
    for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend_ex(&mut self, iter: &StaticVec<T, N, L>) {
        self.extend_from_slice(iter);
    }

    #[inline(always)]
    fn from_iter_ex(iter: &StaticVec<T, N, L>) -> Self {
        Self::new_from_slice(iter)
    }
}

impl<'a, T: 'a + Copy, const N1: usize, const N2: usize, L1>
    ExtendEx<&'a T, StaticVecIterConst<'a, T, N2>> for StaticVec<T, N1, L1>
where
    L1: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L1 as TryFrom<usize>>::Error: Debug,
    <L1 as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    default fn extend_ex(&mut self, iter: StaticVecIterConst<'a, T, N2>) {
        self.extend_from_slice(iter.as_slice());
    }

    #[inline(always)]
    default fn from_iter_ex(iter: StaticVecIterConst<'a, T, N2>) -> Self {
        Self::new_from_slice(iter.as_slice())
    }
}

impl<'a, T: 'a + Copy, const N: usize, L> ExtendEx<&'a T, StaticVecIterConst<'a, T, N>>
    for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend_ex(&mut self, iter: StaticVecIterConst<'a, T, N>) {
        self.extend_from_slice(iter.as_slice());
    }

    #[inline(always)]
    fn from_iter_ex(iter: StaticVecIterConst<'a, T, N>) -> Self {
        Self::new_from_slice(iter.as_slice())
    }
}

impl<'a, T: 'a + Copy, const N: usize, L> ExtendEx<&'a T, core::slice::Iter<'a, T>>
    for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend_ex(&mut self, iter: core::slice::Iter<'a, T>) {
        self.extend_from_slice(iter.as_slice());
    }

    #[inline(always)]
    fn from_iter_ex(iter: core::slice::Iter<'a, T>) -> Self {
        Self::new_from_slice(iter.as_slice())
    }
}

impl<T: Copy, const N: usize, L> ExtendEx<T, core::array::IntoIter<T, N>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend_ex(&mut self, iter: core::array::IntoIter<T, N>) {
        self.extend_from_slice(iter.as_slice());
    }

    #[inline(always)]
    fn from_iter_ex(iter: core::array::IntoIter<T, N>) -> Self {
        Self::new_from_slice(iter.as_slice())
    }
}

impl<'a, T: 'a + Copy, const N: usize, L> ExtendEx<&'a T, &'a [T]> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend_ex(&mut self, iter: &'a [T]) {
        self.extend_from_slice(iter);
    }

    #[inline(always)]
    fn from_iter_ex(iter: &'a [T]) -> Self {
        Self::new_from_slice(iter)
    }
}

impl<T, const N: usize, L> Extend<T> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        <Self as ExtendEx<T, I>>::extend_ex(self, iter);
    }
}

impl<'a, T: 'a + Copy, const N: usize, L> Extend<&'a T> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        <Self as ExtendEx<&'a T, I>>::extend_ex(self, iter);
    }
}

impl<T: Copy, const N: usize, L> From<&[T]> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Creates a new StaticVec instance from the contents of `values`, using
    /// [`new_from_slice`](crate::StaticVec::new_from_slice) internally.
    #[inline(always)]
    fn from(values: &[T]) -> Self {
        Self::new_from_slice(values)
    }
}

impl<T: Copy, const N: usize, L> From<&mut [T]> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Creates a new StaticVec instance from the contents of `values`, using
    /// [`new_from_slice`](crate::StaticVec::new_from_slice) internally.
    #[inline(always)]
    fn from(values: &mut [T]) -> Self {
        Self::new_from_slice(values)
    }
}

impl<T, const N1: usize, const N2: usize, L> From<[T; N1]> for StaticVec<T, N2, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Creates a new StaticVec instance from the contents of `values`, using
    /// [`new_from_array`](crate::StaticVec::new_from_array) internally.
    #[inline(always)]
    default fn from(values: [T; N1]) -> Self {
        Self::new_from_array(values)
    }
}

impl<T, const N: usize> From<[T; N]> for StaticVec<T, N, usize> {
    #[inline(always)]
    fn from(values: [T; N]) -> Self {
        Self::new_from_const_array(values)
    }
}

impl<T: Copy, const N1: usize, const N2: usize, L> From<&[T; N1]> for StaticVec<T, N2, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Creates a new StaticVec instance from the contents of `values`, using
    /// [`new_from_slice`](crate::StaticVec::new_from_slice) internally.
    #[inline(always)]
    default fn from(values: &[T; N1]) -> Self {
        Self::new_from_slice(values)
    }
}

impl<T: Copy, const N: usize, L> From<&[T; N]> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Creates a new StaticVec instance from the contents of `values`, using
    /// [`new_from_slice`](crate::StaticVec::new_from_slice) internally.
    #[inline(always)]
    fn from(values: &[T; N]) -> Self {
        Self::new_from_slice(values)
    }
}

impl<T: Copy, const N1: usize, const N2: usize, L> From<&mut [T; N1]> for StaticVec<T, N2, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Creates a new StaticVec instance from the contents of `values`, using
    /// [`new_from_slice`](crate::StaticVec::new_from_slice) internally.
    #[inline(always)]
    default fn from(values: &mut [T; N1]) -> Self {
        Self::new_from_slice(values)
    }
}

impl<T: Copy, const N: usize, L> From<&mut [T; N]> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Creates a new StaticVec instance from the contents of `values`, using
    /// [`new_from_slice`](crate::StaticVec::new_from_slice) internally.
    #[inline(always)]
    fn from(values: &mut [T; N]) -> Self {
        Self::new_from_slice(values)
    }
}

impl<T, const N1: usize, const N2: usize, L> From<StaticHeap<T, N1>> for StaticVec<T, N2, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    default fn from(heap: StaticHeap<T, N1>) -> StaticVec<T, N2, L> {
        StaticVec::<T, N2, L>::from_iter(heap.data)
    }
}

impl<T, const N: usize> From<StaticHeap<T, N>> for StaticVec<T, N, usize> {
    #[inline(always)]
    fn from(heap: StaticHeap<T, N>) -> StaticVec<T, N, usize> {
        heap.data
    }
}

// TODO update length type when string has generic length type
impl<const N1: usize, const N2: usize> From<StaticString<N1>> for StaticVec<u8, N2, usize> {
    #[inline(always)]
    default fn from(string: StaticString<N1>) -> Self {
        Self::new_from_slice(string.as_bytes())
    }
}

// TODO update length type when string has generic length type
impl<const N: usize> From<StaticString<N>> for StaticVec<u8, N, usize> {
    #[inline(always)]
    fn from(string: StaticString<N>) -> Self {
        string.into_bytes()
    }
}

#[cfg(feature = "alloc")]
#[doc(cfg(feature = "alloc"))]
impl<T, const N: usize, L> From<Vec<T>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Functionally equivalent to [`from_vec`](crate::StaticVec::from_vec).
    #[inline(always)]
    fn from(vec: Vec<T>) -> Self {
        Self::from_vec(vec)
    }
}

impl<T, const N: usize, L> FromIterator<T> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        <Self as ExtendEx<T, I>>::from_iter_ex(iter)
    }
}

impl<'a, T: 'a + Copy, const N: usize, L> FromIterator<&'a T> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn from_iter<I: IntoIterator<Item = &'a T>>(iter: I) -> Self {
        <Self as ExtendEx<&'a T, I>>::from_iter_ex(iter)
    }
}

impl<T: Hash, const N: usize, L> Hash for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

// We implement the various forms of `Index` directly, as after trying out
// deferring to `SliceIndex` for it for a while it proved to to be somewhat
// less performant due to the added indirection.

impl<T, const N: usize, L> Index<L> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = T;
    /// Asserts that `index` is less than the current length of the StaticVec,
    /// and if so returns the value at that position as a constant reference.
    #[inline(always)]
    fn index(&self, index: L) -> &Self::Output {
        // The formatted assertion macros are not const-compatible yet.
        /*
        assert!(
          index < self.length,
          "In StaticVec::index, provided index {} must be less than the current length of {}!",
          index,
          self.length
        );
        */
        assert!(
            index < self.length,
            "Bounds check failure in StaticVec::index!",
        );
        unsafe { self.get_unchecked(index) }
    }
}

impl<T, const N: usize, L> IndexMut<L> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Asserts that `index` is less than the current length of the StaticVec,
    /// and if so returns the value at that position as a mutable reference.
    #[inline(always)]
    fn index_mut(&mut self, index: L) -> &mut Self::Output {
        // The formatted assertion macros are not const-compatible yet.
        /*
        assert!(
          index < self.length,
          "In StaticVec::index_mut, provided index {} must be less than the current length of {}!",
          index,
          self.length
        );
        */
        assert!(
            index < self.length,
            "Bounds check failure in StaticVec::index_mut!",
        );
        unsafe { self.get_unchecked_mut(index) }
    }
}

impl<T, const N: usize, L> Index<Range<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = [T];
    /// Asserts that the lower bound of `index` is less than or equal to its upper bound,
    /// and that its upper bound is less than or equal to the current length of the StaticVec,
    /// and if so returns a constant reference to a slice of elements `index.start..index.end`.
    #[allow(clippy::suspicious_operation_groupings)]
    #[inline(always)]
    fn index(&self, index: Range<L>) -> &Self::Output {
        // This is the part that confuses Clippy.
        assert!(index.start <= index.end && index.end <= self.length);
        unsafe {
            from_raw_parts(
                self.ptr_at_unchecked(index.start),
                (index.end - index.start).try_into().unwrap(),
            )
        }
    }
}

impl<T, const N: usize, L> IndexMut<Range<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Asserts that the lower bound of `index` is less than or equal to its upper bound,
    /// and that its upper bound is less than or equal to the current length of the StaticVec,
    /// and if so returns a mutable reference to a slice of elements `index.start..index.end`.
    #[allow(clippy::suspicious_operation_groupings)]
    #[inline(always)]
    fn index_mut(&mut self, index: Range<L>) -> &mut Self::Output {
        // This is the part that confuses Clippy.
        assert!(index.start <= index.end && index.end <= self.length);
        unsafe {
            from_raw_parts_mut(
                self.mut_ptr_at_unchecked(index.start),
                (index.end - index.start).try_into().unwrap(),
            )
        }
    }
}

impl<T, const N: usize, L> Index<RangeFrom<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = [T];
    /// Asserts that the lower bound of `index` is less than or equal to the
    /// current length of the StaticVec, and if so returns a constant reference
    /// to a slice of elements `index.start()..self.length`.
    #[inline(always)]
    fn index(&self, index: RangeFrom<L>) -> &Self::Output {
        assert!(index.start <= self.length);
        unsafe {
            from_raw_parts(
                self.ptr_at_unchecked(index.start),
                (self.length - index.start).try_into().unwrap(),
            )
        }
    }
}

impl<T, const N: usize, L> IndexMut<RangeFrom<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Asserts that the lower bound of `index` is less than or equal to the
    /// current length of the StaticVec, and if so returns a mutable reference
    /// to a slice of elements `index.start()..self.length`.
    #[inline(always)]
    fn index_mut(&mut self, index: RangeFrom<L>) -> &mut Self::Output {
        assert!(index.start <= self.length);
        unsafe {
            from_raw_parts_mut(
                self.mut_ptr_at_unchecked(index.start),
                (self.length - index.start).try_into().unwrap(),
            )
        }
    }
}

impl<T, const N: usize> Index<RangeFull> for StaticVec<T, N, usize> {
    type Output = [T];
    /// Returns a constant reference to a slice consisting of `0..self.length`
    /// elements of the StaticVec, using [as_slice](crate::StaticVec::as_slice) internally.
    #[inline(always)]
    fn index(&self, _index: RangeFull) -> &Self::Output {
        self.as_slice()
    }
}

impl<T, const N: usize> IndexMut<RangeFull> for StaticVec<T, N, usize> {
    /// Returns a mutable reference to a slice consisting of `0..self.length`
    /// elements of the StaticVec, using [as_mut_slice](crate::StaticVec::as_mut_slice) internally.
    #[inline(always)]
    fn index_mut(&mut self, _index: RangeFull) -> &mut Self::Output {
        self.as_mut_slice()
    }
}

impl<T, const N: usize, L> Index<RangeInclusive<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = [T];
    /// Asserts that the lower bound of `index` is less than or equal to its upper bound,
    /// and that its upper bound is less than the current length of the StaticVec,
    /// and if so returns a constant reference to a slice of elements `index.start()..=index.end()`.
    #[inline(always)]
    fn index(&self, index: RangeInclusive<L>) -> &Self::Output {
        let start = *index.start();
        let end = *index.end();
        assert!(start <= end && end < self.length);
        unsafe {
            from_raw_parts(
                self.ptr_at_unchecked(start),
                ((end + L::ONE) - start).try_into().unwrap(),
            )
        }
    }
}

impl<T, const N: usize, L> IndexMut<RangeInclusive<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Asserts that the lower bound of `index` is less than or equal to its upper bound,
    /// and that its upper bound is less than the current length of the StaticVec,
    /// and if so returns a mutable reference to a slice of elements `index.start()..=index.end()`.
    #[inline(always)]
    fn index_mut(&mut self, index: RangeInclusive<L>) -> &mut Self::Output {
        let start = *index.start();
        let end = *index.end();
        assert!(start <= end && end < self.length);
        unsafe {
            from_raw_parts_mut(
                self.mut_ptr_at_unchecked(start),
                ((end + L::ONE) - start).try_into().unwrap(),
            )
        }
    }
}

impl<T, const N: usize, L> Index<RangeTo<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = [T];
    /// Asserts that the upper bound of `index` is less than or equal to the
    /// current length of the StaticVec, and if so returns a constant reference
    /// to a slice of elements `0..index.end`.
    #[inline(always)]
    fn index(&self, index: RangeTo<L>) -> &Self::Output {
        assert!(index.end <= self.length);
        unsafe { from_raw_parts(self.as_ptr(), index.end.try_into().unwrap()) }
    }
}

impl<T, const N: usize, L> IndexMut<RangeTo<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Asserts that the upper bound of `index` is less than or equal to the
    /// current length of the StaticVec, and if so returns a constant reference
    /// to a slice of elements `0..index.end`.
    #[inline(always)]
    fn index_mut(&mut self, index: RangeTo<L>) -> &mut Self::Output {
        assert!(index.end <= self.length);
        unsafe { from_raw_parts_mut(self.as_mut_ptr(), index.end.try_into().unwrap()) }
    }
}

impl<T, const N: usize, L> Index<RangeToInclusive<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type Output = [T];
    /// Asserts that the upper bound of `index` is less than the
    /// current length of the StaticVec, and if so returns a constant reference
    /// to a slice of elements `0..=index.end`.
    #[inline(always)]
    fn index(&self, index: RangeToInclusive<L>) -> &Self::Output {
        assert!(index.end < self.length);
        unsafe { from_raw_parts(self.as_ptr(), index.end.try_into().unwrap() + 1) }
    }
}

impl<T, const N: usize, L> IndexMut<RangeToInclusive<L>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Asserts that the upper bound of `index` is less than the
    /// current length of the StaticVec, and if so returns a constant reference
    /// to a slice of elements `0..=index.end`.
    #[inline(always)]
    fn index_mut(&mut self, index: RangeToInclusive<L>) -> &mut Self::Output {
        assert!(index.end < self.length);
        unsafe { from_raw_parts_mut(self.as_mut_ptr(), index.end.try_into().unwrap() + 1) }
    }
}

#[allow(clippy::from_over_into)]
#[cfg(feature = "alloc")]
#[doc(cfg(feature = "alloc"))]
impl<T, const N: usize, L> Into<Vec<T>> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    /// Functionally equivalent to [`into_vec`](crate::StaticVec::into_vec).
    #[inline(always)]
    fn into(self) -> Vec<T> {
        self.into_vec()
    }
}

impl<'a, T: 'a, L, const N: usize> IntoIterator for &'a StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type IntoIter = StaticVecIterConst<'a, T, N>;
    type Item = &'a T;
    /// Returns a [`StaticVecIterConst`](crate::iterators::StaticVecIterConst) over the StaticVec's
    /// inhabited area.
    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: 'a, L, const N: usize> IntoIterator for &'a mut StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type IntoIter = StaticVecIterMut<'a, T, N>;
    type Item = &'a mut T;
    /// Returns a [`StaticVecIterMut`](crate::iterators::StaticVecIterMut) over the StaticVec's
    /// inhabited area.
    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T, L, const N: usize> IntoIterator for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize> + Ord,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    type IntoIter = StaticVecIntoIter<T, N>;
    type Item = T;
    /// Returns a by-value [`StaticVecIntoIter`](crate::iterators::StaticVecIntoIter) over the
    /// StaticVec's inhabited area, which consumes the StaticVec.
    #[inline(always)]
    fn into_iter(self) -> Self::IntoIter {
        let old_length: usize = self.length.try_into().unwrap();
        StaticVecIntoIter {
            start: 0,
            end: old_length,
            data: {
                // Copy the inhabited part of `self` into the iterator.
                let mut data = StaticVec::new_data_uninit();
                unsafe {
                    // The `MaybeUninit` wrapping prevents the values from being dropped locally, which
                    // is necessary since again they're being copied into the iterator.
                    MaybeUninit::new(self)
                        .assume_init_ref()
                        .as_ptr()
                        .copy_to_nonoverlapping(StaticVec::first_ptr_mut(&mut data), old_length)
                };
                data
            },
        }
    }
}

impl<T: Ord, L, const N: usize> Ord for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline(always)]
    fn cmp(&self, other: &Self) -> Ordering {
        Ord::cmp(self.as_slice(), other.as_slice())
    }
}

impl_partial_eq_with_as_slice!(StaticVec<T1, N1, L1>, StaticVec<T2, N2, L2>);
impl_partial_eq_with_as_slice!(StaticVec<T1, N1, L1>, &StaticVec<T2, N2, L2>);
impl_partial_eq_with_as_slice!(StaticVec<T1, N1, L1>, &mut StaticVec<T2, N2, L2>);
impl_partial_eq_with_as_slice!(&StaticVec<T1, N1, L1>, StaticVec<T2, N2, L2>);
impl_partial_eq_with_as_slice!(&mut StaticVec<T1, N1, L1>, StaticVec<T2, N2, L2>);
impl_partial_eq_with_get_unchecked!([T1; N1], StaticVec<T2, N2, L2>);
impl_partial_eq_with_get_unchecked!([T1; N1], &StaticVec<T2, N2, L2>);
impl_partial_eq_with_get_unchecked!([T1; N1], &mut StaticVec<T2, N2, L2>);
impl_partial_eq_with_get_unchecked!(&[T1; N1], StaticVec<T2, N2, L2>);
impl_partial_eq_with_get_unchecked!(&mut [T1; N1], StaticVec<T2, N2, L2>);
impl_partial_eq_with_equals_no_deref!([T1], StaticVec<T2, N, L>);
impl_partial_eq_with_equals_no_deref!([T1], &StaticVec<T2, N, L>);
impl_partial_eq_with_equals_no_deref!([T1], &mut StaticVec<T2, N, L>);
impl_partial_eq_with_equals_deref!(&[T1], StaticVec<T2, N, L>);
impl_partial_eq_with_equals_deref!(&mut [T1], StaticVec<T2, N, L>);
impl_partial_ord_with_as_slice!(StaticVec<T1, N1, L1>, StaticVec<T2, N2, L2>);
impl_partial_ord_with_as_slice!(StaticVec<T1, N1, L1>, &StaticVec<T2, N2, L2>);
impl_partial_ord_with_as_slice!(StaticVec<T1, N1, L1>, &mut StaticVec<T2, N2, L2>);
impl_partial_ord_with_as_slice!(&StaticVec<T1, N1, L1>, StaticVec<T2, N2, L2>);
impl_partial_ord_with_as_slice!(&mut StaticVec<T1, N1, L1>, StaticVec<T2, N2, L2>);
impl_partial_ord_with_get_unchecked!([T1; N1], StaticVec<T2, N2, L>);
impl_partial_ord_with_get_unchecked!([T1; N1], &StaticVec<T2, N2, L>);
impl_partial_ord_with_get_unchecked!([T1; N1], &mut StaticVec<T2, N2, L>);
impl_partial_ord_with_get_unchecked!(&[T1; N1], StaticVec<T2, N2, L>);
impl_partial_ord_with_get_unchecked!(&mut [T1; N1], StaticVec<T2, N2, L>);
impl_partial_ord_with_as_slice_against_slice!([T1], StaticVec<T2, N, L>);
impl_partial_ord_with_as_slice_against_slice!([T1], &StaticVec<T2, N, L>);
impl_partial_ord_with_as_slice_against_slice!([T1], &mut StaticVec<T2, N, L>);
impl_partial_ord_with_as_slice_against_slice!(&[T1], StaticVec<T2, N, L>);
impl_partial_ord_with_as_slice_against_slice!(&mut [T1], StaticVec<T2, N, L>);

impl<const N: usize, L> fmt::Write for StaticVec<u8, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
{
    #[inline]
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // This is just exactly `try_extend_from_slice`, except with the specific `Result` type
        // that this particular trait method calls for.
        let old_length = self.length;
        let added_length: L = s.len().try_into().unwrap();
        if TryInto::<L>::try_into(N).unwrap() - old_length < added_length {
            return Err(fmt::Error);
        }
        unsafe {
            s.as_ptr().copy_to_nonoverlapping(
                self.mut_ptr_at_unchecked(old_length),
                added_length.try_into().unwrap(),
            );
            self.set_len(old_length + added_length);
        }
        Ok(())
    }
}

#[cfg(feature = "serde")]
#[doc(cfg(feature = "serde"))]
impl<'de, T, const N: usize, L> Deserialize<'de> for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
    T: Deserialize<'de>,
{
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StaticVecVisitor<'de, T, const N: usize, L>(
            PhantomData<(&'de (), T)>,
            PhantomData<L>,
        );

        impl<'de, T, const N: usize, L> Visitor<'de> for StaticVecVisitor<'de, T, N, L>
        where
            T: Deserialize<'de>,
            L: Number + TryFrom<usize> + TryInto<usize>,
            <L as TryFrom<usize>>::Error: Debug,
            <L as TryInto<usize>>::Error: Debug,
        {
            type Value = StaticVec<T, N, L>;

            #[inline(always)]
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "no more than {} items", N)
            }

            #[inline]
            fn visit_seq<SA>(self, mut seq: SA) -> Result<Self::Value, SA::Error>
            where
                SA: SeqAccess<'de>,
            {
                let mut res = Self::Value::new();
                while res.length < N.try_into().unwrap() {
                    if let Some(val) = seq.next_element()? {
                        unsafe { res.push_unchecked(val) };
                    } else {
                        break;
                    }
                }
                Ok(res)
            }
        }
        deserializer.deserialize_seq(StaticVecVisitor::<T, N, L>(PhantomData, PhantomData))
    }
}

#[cfg(feature = "serde")]
#[doc(cfg(feature = "serde"))]
impl<T, const N: usize, L> Serialize for StaticVec<T, N, L>
where
    L: Number + TryFrom<usize> + TryInto<usize>,
    <L as TryFrom<usize>>::Error: Debug,
    <L as TryInto<usize>>::Error: Debug,
    T: Serialize,
{
    #[inline(always)]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self)
    }
}

#[cfg(test)]
mod test {
    use crate::StaticVec;

    #[test]
    fn test_extend_drop_no_panic() {
        let mut a = StaticVec::<u64, 6, usize>::new();
        let mut b = StaticVec::<u64, 6, usize>::new();

        a.extend_from_slice(&[1, 2]);
        b.extend_from_slice(&[3, 4, 5]);

        a.extend(b);

        assert_eq!(a, [1, 2, 3, 4, 5]);
    }
}
