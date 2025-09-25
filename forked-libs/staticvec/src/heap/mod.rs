use core::mem::swap;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use self::heap_helpers::StaticHeapHole;
pub use self::heap_helpers::StaticHeapPeekMut;
pub use self::heap_iterators::{StaticHeapDrainSorted, StaticHeapIntoIterSorted};
use crate::iterators::{StaticVecDrain, StaticVecIterConst, StaticVecIterMut};
use crate::StaticVec;

mod heap_helpers;
mod heap_iterators;
mod heap_trait_impls;

/// A priority queue implemented as a binary heap, built around an instance of `StaticVec<T, N>`.
///
/// `StaticHeap`, as well as the associated iterator and helper structs for it are direct
/// adaptations of the ones found in the `std::collections::binary_heap` module (including
/// most of the documentation, at least for the functions that exist in both implementations).
///
/// It is a logic error for an item to be modified in such a way that the
/// item's ordering relative to any other item, as determined by the `Ord`
/// trait, changes while it is in the heap. This is normally only possible
/// through `Cell`, `RefCell`, global state, I/O, or unsafe code.
///
/// # Examples
///
/// ```
/// use staticvec::StaticHeap;
///
/// let mut heap = StaticHeap::<i32, 4>::new();
///
/// // We can use peek to look at the next item in the heap. In this case,
/// // there's no items in there yet so we get None.
/// assert_eq!(heap.peek(), None);
///
/// // Let's add some scores...
/// heap.push(1);
/// heap.push(5);
/// heap.push(2);
///
/// // Now peek shows the most important item in the heap.
/// assert_eq!(heap.peek(), Some(&5));
///
/// // We can check the length of a heap.
/// assert_eq!(heap.len(), 3);
///
/// // We can iterate over the items in the heap, although they are returned in
/// // a random order.
/// for x in &heap {
///   println!("{}", x);
/// }
///
/// // If we instead pop these scores, they should come back in order.
/// assert_eq!(heap.pop(), Some(5));
/// assert_eq!(heap.pop(), Some(2));
/// assert_eq!(heap.pop(), Some(1));
/// assert_eq!(heap.pop(), None);
///
/// // We can clear the heap of any remaining items.
/// heap.clear();
///
/// // The heap should now be empty.
/// assert!(heap.is_empty())
/// ```
///
/// ## Min-heap
///
/// Either `core::cmp::Reverse` or a custom `Ord` implementation can be used to
/// make `StaticHeap` a min-heap. This makes `heap.pop()` return the smallest
/// value instead of the greatest one.
///
/// ```
/// use staticvec::StaticHeap;
/// use core::cmp::Reverse;
///
/// // Wrap the values in `Reverse`.
/// let mut heap = StaticHeap::from([Reverse(1), Reverse(5), Reverse(2)]);
///
/// // If we pop these scores now, they should come back in the reverse order.
/// assert_eq!(heap.pop(), Some(Reverse(1)));
/// assert_eq!(heap.pop(), Some(Reverse(2)));
/// assert_eq!(heap.pop(), Some(Reverse(5)));
/// assert_eq!(heap.pop(), None);
/// ```
///
/// # Time complexity
///
/// | [push] | [pop]    | [peek]/[peek\_mut] |
/// |--------|----------|--------------------|
/// | O(1)~  | O(log n) | O(1)               |
///
/// The value for `push` is an expected cost; the method documentation gives a
/// more detailed analysis.
///
/// [push]: #method.push
/// [pop]: #method.pop
/// [peek]: #method.peek
/// [peek\_mut]: #method.peek_mut
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
pub struct StaticHeap<T, const N: usize> {
    pub(crate) data: StaticVec<T, N>,
}

impl<T: Ord, const N: usize> StaticHeap<T, N> {
    /// Creates an empty StaticHeap as a max-heap.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::StaticHeap;
    /// let mut heap = StaticHeap::<i32, 2>::new();
    /// heap.push(4);
    /// ```
    #[inline(always)]
    pub const fn new() -> StaticHeap<T, N> {
        StaticHeap {
            data: StaticVec::new(),
        }
    }

    /// Returns a mutable reference to the greatest item in the StaticHeap, or
    /// `None` if it is empty.
    ///
    /// Note: If the `StaticHeapPeekMut` value is leaked, the heap may be in an
    /// inconsistent state.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::StaticHeap;
    /// let mut heap = StaticHeap::<i32, 4>::new();
    /// assert!(heap.peek_mut().is_none());
    /// heap.push(1);
    /// heap.push(5);
    /// heap.push(2);
    /// {
    ///   let mut val = heap.peek_mut().unwrap();
    ///   *val = 0;
    /// }
    /// assert_eq!(heap.peek(), Some(&2));
    /// ```
    ///
    /// # Time complexity
    ///
    /// Cost is O(1) in the worst case.
    #[inline(always)]
    pub fn peek_mut(&mut self) -> Option<StaticHeapPeekMut<'_, T, N>> {
        if self.is_empty() {
            None
        } else {
            Some(StaticHeapPeekMut {
                heap: self,
                sift: true,
            })
        }
    }

    /// Pops a value from the end of the StaticHeap and returns it directly without asserting that
    /// the StaticHeap's current length is greater than 0.
    ///
    /// # Safety
    ///
    /// It is up to the caller to ensure that the StaticHeap contains at least one
    /// element prior to using this function. Failure to do so will result in reading
    /// from uninitialized memory.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::from([1, 3]);
    /// unsafe {
    ///   assert_eq!(heap.pop_unchecked(), 3);
    ///   assert_eq!(heap.pop_unchecked(), 1);
    /// }
    /// ```
    ///
    /// # Time complexity
    ///
    /// The worst case cost of `pop_unchecked` on a heap containing *n* elements is O(log n).
    #[inline(always)]
    pub unsafe fn pop_unchecked(&mut self) -> T {
        let mut res = self.data.pop_unchecked();
        if self.is_not_empty() {
            swap(&mut res, self.data.get_unchecked_mut(0));
            self.sift_down_to_bottom(0);
        }
        res
    }

    /// Removes the greatest item from the StaticHeap and returns it, or `None` if it
    /// is empty.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::from([1, 3]);
    /// assert_eq!(heap.pop(), Some(3));
    /// assert_eq!(heap.pop(), Some(1));
    /// assert_eq!(heap.pop(), None);
    /// ```
    ///
    /// # Time complexity
    ///
    /// The worst case cost of `pop` on a heap containing *n* elements is O(log n).
    #[inline(always)]
    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            None
        } else {
            Some(unsafe { self.pop_unchecked() })
        }
    }

    /// Pushes a value onto the StaticHeap without asserting that
    /// its current length is less than `self.capacity()`.
    ///
    /// # Safety
    ///
    /// It is up to the caller to ensure that the length of the StaticHeap
    /// prior to using this function is less than `self.capacity()`.
    /// Failure to do so will result in writing to an out-of-bounds memory region.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::StaticHeap;
    /// let mut heap = StaticHeap::<i32, 3>::new();
    /// unsafe {
    ///   heap.push_unchecked(3);
    ///   heap.push_unchecked(5);
    ///   heap.push_unchecked(1);
    /// }
    /// assert_eq!(heap.len(), 3);
    /// assert_eq!(heap.peek(), Some(&5));
    /// ```
    ///
    /// # Time complexity
    ///
    /// The expected cost of `push_unchecked`, averaged over every possible ordering of
    /// the elements being pushed, and over a sufficiently large number of
    /// pushes, is O(1). This is the most meaningful cost metric when pushing
    /// elements that are *not* already in any sorted pattern.
    ///
    /// The time complexity degrades if elements are pushed in predominantly
    /// ascending order. In the worst case, elements are pushed in ascending
    /// sorted order and the amortized cost per push is O(log n) against a heap
    /// containing *n* elements.
    ///
    /// The worst case cost of a *single* call to `push_unchecked` is O(n).
    #[inline(always)]
    pub unsafe fn push_unchecked(&mut self, item: T) {
        let old_length = self.len();
        self.data.push_unchecked(item);
        self.sift_up(0, old_length);
    }

    /// Pushes an item onto the StaticHeap, panicking if the underlying StaticVec
    /// instance is already at maximum capacity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::StaticHeap;
    /// let mut heap = StaticHeap::<i32, 5>::new();
    /// heap.push(3);
    /// heap.push(5);
    /// heap.push(1);
    /// assert_eq!(heap.len(), 3);
    /// assert_eq!(heap.peek(), Some(&5));
    /// ```
    ///
    /// # Time complexity
    ///
    /// The expected cost of `push`, averaged over every possible ordering of
    /// the elements being pushed, and over a sufficiently large number of
    /// pushes, is O(1). This is the most meaningful cost metric when pushing
    /// elements that are *not* already in any sorted pattern.
    ///
    /// The time complexity degrades if elements are pushed in predominantly
    /// ascending order. In the worst case, elements are pushed in ascending
    /// sorted order and the amortized cost per push is O(log n) against a heap
    /// containing *n* elements.
    ///
    /// The worst case cost of a *single* call to `push` is O(n).
    #[inline(always)]
    pub fn push(&mut self, item: T) {
        // Deferring to our own `push_unchecked` which defers to `StaticVec::push_unchecked`
        // is slower here than just calling `StaticVec::push` which calls `StaticVec::push_unchecked`
        // anyways.
        let old_length = self.len();
        self.data.push(item);
        self.sift_up(0, old_length);
    }

    /// Consumes the StaticHeap and returns a StaticVec in sorted (ascending) order.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::<i32, 8>::from([1, 2, 4, 5, 7]);
    /// heap.push(6);
    /// heap.push(3);
    /// let vec = heap.into_sorted_staticvec();
    /// assert_eq!(vec, [1, 2, 3, 4, 5, 6, 7]);
    /// ```
    #[inline]
    pub fn into_sorted_staticvec(mut self) -> StaticVec<T, N> {
        let mut end = self.len();
        while end > 1 {
            end -= 1;
            self.data.swap(0, end);
            self.sift_down_range(0, end);
        }
        self.into_staticvec()
    }

    // The implementations of sift_up and sift_down use unsafe blocks in
    // order to move an element out of the vector (leaving behind a
    // hole), shift along the others and move the removed element back into the
    // vector at the final location of the hole.
    // The `StaticHeapHole` type is used to represent this, and make sure
    // the hole is filled back at the end of its scope, even on panic.
    // Using a hole reduces the constant factor compared to using swaps,
    // which involves twice as many moves.
    #[inline]
    fn sift_up(&mut self, start: usize, position: usize) {
        unsafe {
            // Take out the value at `position` and create a hole.
            let mut hole = StaticHeapHole::new(&mut self.data, position);
            while hole.pos() > start {
                let parent = (hole.pos() - 1) / 2;
                if hole.elt() <= hole.get(parent) {
                    break;
                }
                hole.move_to(parent);
            }
        }
    }

    /// Takes an element from `position` and moves it down the heap,
    /// while its children are larger.
    #[inline]
    fn sift_down_range(&mut self, position: usize, end: usize) {
        unsafe {
            let mut hole = StaticHeapHole::new(&mut self.data, position);
            let mut child = 2 * position + 1;
            while child < end {
                let right = child + 1;
                // compare with the greater of the two children
                if right < end && hole.get(child) <= hole.get(right) {
                    child = right;
                }
                // if we are already in order, stop.
                if hole.elt() >= hole.get(child) {
                    break;
                }
                hole.move_to(child);
                child = 2 * hole.pos() + 1;
            }
        }
    }

    /// Takes an element from `position` and moves it all the way down the heap,
    /// then sifts it up to its position.
    ///
    /// Note: This is faster when the element is known to be large / should
    /// be closer to the bottom.
    #[inline]
    fn sift_down_to_bottom(&mut self, mut position: usize) {
        let end = self.len();
        let start = position;
        unsafe {
            let mut hole = StaticHeapHole::new(&mut self.data, position);
            let mut child = 2 * position + 1;
            while child < end {
                let right = child + 1;
                // compare with the greater of the two children
                if right < end && hole.get(child) <= hole.get(right) {
                    child = right;
                }
                hole.move_to(child);
                child = 2 * hole.pos() + 1;
            }
            position = hole.position;
        }
        self.sift_up(start, position);
    }

    #[inline(always)]
    fn rebuild(&mut self) {
        let mut n = self.len() / 2;
        while n > 0 {
            n -= 1;
            self.sift_down_range(n, self.len());
        }
    }

    /// Appends `self.remaining_capacity()` (or as many as available) items from `other` to `self`.
    /// The appended items (if any) will no longer exist in `other` afterwards (which is to say,
    /// `other` will be left empty.)
    ///
    /// The `N2` parameter does not need to be provided explicitly, and can be inferred directly from
    /// the constant `N2` constraint of `other` (which may or may not be the same as the `N`
    /// constraint of `self`.)
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// // We give the two heaps arbitrary capacities for the sake of the example.
    /// let mut a = StaticHeap::<i32, 9>::from([-10, 1, 2, 3, 3]);
    /// let mut b = StaticHeap::<i32, 18>::from([-20, 5, 43]);
    /// a.append(&mut b);
    /// assert_eq!(a.into_sorted_staticvec(), [-20, -10, 1, 2, 3, 3, 5, 43]);
    /// assert!(b.is_empty());
    /// ```
    #[inline(always)]
    pub fn append<const N2: usize>(&mut self, other: &mut StaticHeap<T, N2>) {
        if other.is_empty() {
            return;
        }
        self.data.append(&mut other.data);
        self.rebuild();
    }

    /// Returns an iterator which retrieves elements in heap order.
    /// The retrieved elements are removed from the original heap.
    /// The remaining elements will be removed on drop in heap order.
    ///
    /// Note:
    /// * `drain_sorted()` is O(n log n); much slower than `drain()`. You should use the latter for
    ///   most cases.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::from([1, 2, 3, 4, 5]);
    /// assert_eq!(heap.len(), 5);
    /// drop(heap.drain_sorted()); // removes all elements in heap order
    /// assert_eq!(heap.len(), 0);
    /// ```
    #[inline(always)]
    pub fn drain_sorted(&mut self) -> StaticHeapDrainSorted<'_, T, N> {
        StaticHeapDrainSorted { inner: self }
    }
}

impl<T, const N: usize> StaticHeap<T, N> {
    /// Returns an iterator visiting all values in the StaticHeap's underlying StaticVec, in
    /// arbitrary order.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let heap = StaticHeap::from(staticvec![1, 2, 3, 4]);
    /// // Print 1, 2, 3, 4 in arbitrary order
    /// for x in heap.iter() {
    ///   println!("{}", x);
    /// }
    /// ```
    #[inline(always)]
    pub fn iter(&self) -> StaticVecIterConst<'_, T, N> {
        self.data.iter()
    }

    /// Returns a mutable iterator visiting all values in the StaticHeap's underlying StaticVec, in
    /// arbitrary order.
    ///
    /// **Note:** Mutating the elements in a StaticHeap may cause it to become unbalanced.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::from([1, 2, 3, 4]);
    /// for i in heap.iter_mut() {
    ///   *i *= 2;
    /// }
    /// // Prints "[2, 4, 6, 8]", but in arbitrary order
    /// println!("{:?}", heap);
    /// ```
    #[inline(always)]
    pub fn iter_mut(&mut self) -> StaticVecIterMut<'_, T, N> {
        self.data.iter_mut()
    }

    /// Returns an iterator which retrieves elements in heap order.
    /// This method consumes the original StaticHeap.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let heap = StaticHeap::from([1, 2, 3, 4, 5]);
    /// assert_eq!(
    ///   heap.into_iter_sorted().take(2).collect::<StaticVec<_, 3>>(), staticvec![5, 4]
    /// );
    /// ```
    #[inline(always)]
    pub fn into_iter_sorted(self) -> StaticHeapIntoIterSorted<T, N> {
        StaticHeapIntoIterSorted { inner: self }
    }

    /// Returns the greatest item in the StaticHeap, or `None` if it is empty.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::<i32, 7>::new();
    /// assert_eq!(heap.peek(), None);
    /// heap.push(1);
    /// heap.push(5);
    /// heap.push(2);
    /// assert_eq!(heap.peek(), Some(&5));
    /// ```
    ///
    /// # Time complexity
    ///
    /// Cost is O(1) in the worst case.
    #[inline(always)]
    pub fn peek(&self) -> Option<&T> {
        self.data.get(0)
    }

    /// Returns the maximum number of elements the StaticHeap can hold.
    /// This is always equivalent to its constant generic `N` parameter.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::<i32, 100>::new();
    /// assert!(heap.capacity() >= 100);
    /// heap.push(4);
    /// ```
    #[inline(always)]
    pub const fn capacity(&self) -> usize {
        self.data.capacity()
    }

    /// Returns the remaining capacity (which is to say, `self.capacity() - self.len()`) of the
    /// StaticHeap.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::<i32, 100>::new();
    /// heap.push(1);
    /// assert_eq!(heap.remaining_capacity(), 99);
    /// ```
    #[inline(always)]
    pub fn remaining_capacity(&self) -> usize {
        self.data.remaining_capacity()
    }

    /// Returns the total size of the inhabited part of the StaticHeap (which may be zero if it has a
    /// length of zero or contains ZSTs) in bytes. Specifically, the return value of this function
    /// amounts to a calculation of `size_of::<T>() * self.length`.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let x = StaticHeap::<u8, 8>::from([1, 2, 3, 4, 5, 6, 7, 8]);
    /// assert_eq!(x.size_in_bytes(), 8);
    /// let y = StaticHeap::<u16, 8>::from([1, 2, 3, 4, 5, 6, 7, 8]);
    /// assert_eq!(y.size_in_bytes(), 16);
    /// let z = StaticHeap::<u32, 8>::from([1, 2, 3, 4, 5, 6, 7, 8]);
    /// assert_eq!(z.size_in_bytes(), 32);
    /// let w = StaticHeap::<u64, 8>::from([1, 2, 3, 4, 5, 6, 7, 8]);
    /// assert_eq!(w.size_in_bytes(), 64);
    /// ```
    #[inline(always)]
    pub fn size_in_bytes(&self) -> usize {
        self.data.size_in_bytes()
    }

    /// Consumes the StaticHeap and returns the underlying StaticVec
    /// in arbitrary order.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let heap = StaticHeap::from(staticvec![1, 2, 3, 4, 5, 6, 7]);
    /// let vec = heap.into_staticvec();
    /// // Will print in some order
    /// for x in &vec {
    ///   println!("{}", x);
    /// }
    /// ```
    #[inline(always)]
    pub fn into_staticvec(self) -> StaticVec<T, N> {
        self.data
    }

    /// Returns the length of the StaticHeap.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let heap = StaticHeap::from(staticvec![1, 3]);
    /// assert_eq!(heap.len(), 2);
    /// ```
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns true if the current length of the StaticHeap is 0.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::<i32, 28>::new();
    /// assert!(heap.is_empty());
    /// ```
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns true if the current length of the StaticHeap is greater than 0.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::<i32, 2>::new();
    /// heap.push(1);
    /// assert!(heap.is_not_empty());
    /// ```
    // Clippy wants `!is_empty()` for this, but I prefer it as-is. My question is though, does it
    // actually know that we have an applicable `is_empty()` function, or is it just guessing? I'm not
    // sure.
    #[allow(clippy::len_zero)]
    #[inline(always)]
    pub fn is_not_empty(&self) -> bool {
        self.len() > 0
    }

    /// Returns true if the current length of the StaticHeap is equal to its capacity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::<i32, 4>::new();
    /// heap.push(3);
    /// heap.push(5);
    /// heap.push(1);
    /// heap.push(2);
    /// assert!(heap.is_full());
    /// ```
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.len() == N
    }

    /// Returns true if the current length of the StaticHeap is less than its capacity.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::<i32, 4>::new();
    /// heap.push(3);
    /// heap.push(5);
    /// heap.push(1);
    /// assert!(heap.is_not_full());
    /// ```
    #[inline(always)]
    pub fn is_not_full(&self) -> bool {
        self.len() < N
    }

    /// Clears the StaticHeap, returning an iterator over the removed elements.
    ///
    /// The elements are removed in arbitrary order.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::from(staticvec![1, 3]);
    /// assert!(heap.is_not_empty());
    /// for x in heap.drain() {
    ///   println!("{}", x);
    /// }
    /// assert!(heap.is_empty());
    /// ```
    #[inline(always)]
    pub fn drain(&mut self) -> StaticVecDrain<'_, T, N> {
        self.data.drain_iter(..)
    }

    /// Drops all items from the StaticHeap.
    ///
    /// # Examples
    ///
    /// Basic usage:
    /// ```
    /// # use staticvec::*;
    /// let mut heap = StaticHeap::from(staticvec![1, 3]);
    /// assert!(heap.is_not_empty());
    /// heap.clear();
    /// assert!(heap.is_empty());
    /// ```
    #[inline(always)]
    pub fn clear(&mut self) {
        self.data.clear();
    }
}
