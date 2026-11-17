use std::{
    alloc::Layout,
    fmt::{self, Debug, Formatter},
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
    slice,
};

use crate::{Allocator, Box};

/// A fixed-size list in the arena: a pointer and a length, with no spare capacity.
///
/// A list never grows. Build it in an ordinary [`Vec`] or from an iterator or array of known length,
/// then freeze it into the arena in one allocation of exactly its size. It derefs to a slice, and
/// [`truncate`](List::truncate) shortens it in place.
///
/// Like [`Box`], a list is never dropped, and it refuses to compile for an item type that needs
/// [`Drop`]:
///
/// ```compile_fail
/// use luck_allocator::{Allocator, List};
///
/// let allocator = Allocator::new();
/// let list = List::from_array_in([String::from("leaks")], &allocator);
/// ```
#[derive(PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct List<'alloc, T>(Box<'alloc, [T]>);

impl<'alloc, T> List<'alloc, T> {
    /// An empty list, which allocates nothing.
    #[must_use]
    pub fn new() -> Self {
        Self(Box::new_empty_boxed_slice())
    }

    /// # Panics
    ///
    /// Panics if `items` yields fewer items than its `len()` promised.
    pub fn from_iter_in<I>(items: I, allocator: &'alloc Allocator) -> Self
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let items = items.into_iter();
        if items.len() == 0 {
            return Self::new();
        }
        let slice = allocator.arena().alloc_slice_fill_iter(items);
        // SAFETY: `slice` is in the arena and every item in it is initialized.
        Self(unsafe { Box::from_non_null(NonNull::from(slice)) })
    }

    pub fn from_array_in<const N: usize>(items: [T; N], allocator: &'alloc Allocator) -> Self {
        if N == 0 {
            return Self::new();
        }
        let array = NonNull::from(allocator.alloc(items));
        // SAFETY: `array` is in the arena and holds `N` initialized items.
        Self(unsafe { Box::from_non_null(NonNull::slice_from_raw_parts(array.cast::<T>(), N)) })
    }

    /// Moves `items[at..]` into the arena, leaving `items[..at]` behind, as [`Vec::split_off`]
    /// does.
    ///
    /// # Panics
    ///
    /// Panics if `at > items.len()`.
    pub fn split_off_in(items: &mut Vec<T>, at: usize, allocator: &'alloc Allocator) -> Self {
        let tail = &items[at..];
        let count = tail.len();
        // A `memcpy` call only beats moving item by item from about 512 bytes.
        if size_of_val(tail) < 512 {
            return Self::from_iter_in(items.drain(at..), allocator);
        }
        let destination = allocator.alloc_layout(Layout::for_value(tail)).cast::<T>();
        // SAFETY: `destination` is a fresh arena allocation with room for `count` items, so it
        // can't overlap `items`. Lowering the length of `items` to `at` hands the copied items
        // over to the list without dropping them.
        unsafe {
            ptr::copy_nonoverlapping(tail.as_ptr(), destination.as_ptr(), count);
            items.set_len(at);
            Self(Box::from_non_null(NonNull::slice_from_raw_parts(destination, count)))
        }
    }

    /// Keeps the first `new_length` items. A list already that short stays as it is.
    pub fn truncate(&mut self, new_length: usize) {
        if new_length < self.len() {
            let first = Box::as_non_null(&self.0).cast::<T>();
            // SAFETY: The first `new_length` items are initialized and live in the arena for
            // `'alloc`. The items cut off need no drop.
            self.0 =
                unsafe { Box::from_non_null(NonNull::slice_from_raw_parts(first, new_length)) };
        }
    }
}

impl<T> Default for List<'_, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Deref for List<'_, T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> DerefMut for List<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<T: Debug> Debug for List<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&**self, f)
    }
}

impl<'list, T> IntoIterator for &'list List<'_, T> {
    type Item = &'list T;
    type IntoIter = slice::Iter<'list, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'list, T> IntoIterator for &'list mut List<'_, T> {
    type Item = &'list mut T;
    type IntoIter = slice::IterMut<'list, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
