#![expect(unsafe_code, reason = "a list is a raw pointer, a length, and a capacity in the arena")]

use std::{
    alloc::Layout,
    fmt::{self, Debug, Formatter},
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem,
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
    slice,
};

#[cfg(feature = "track_allocations")]
use crate::AllocationCounts;
use crate::Allocator;

/// A list in the arena: a pointer, a length, and a capacity, in 16 bytes.
///
/// Build a list in an ordinary [`Vec`] or from an iterator or array of known length, and it's frozen
/// into the arena in one allocation of exactly its size, or reserve room for one up front with
/// [`with_capacity_in`](List::with_capacity_in). [`push`](List::push) and
/// [`insert`](List::insert) grow it, moving it to an arena allocation of twice the capacity when
/// it's full, so a list built an item at a time takes amortized constant time per item and less
/// than four times its final size in the arena. [`truncate`](List::truncate) shortens it in place.
/// It derefs to a slice, and [`into_iter`](List::into_iter) hands its items back by value.
///
/// Like [`Box`](crate::Box), a list is never dropped, and it refuses to compile for an item type
/// that needs [`Drop`]:
///
/// ```compile_fail
/// use luck_allocator::{Allocator, List};
///
/// let allocator = Allocator::new();
/// let list = List::from_array_in([String::from("leaks")], &allocator);
/// ```
pub struct List<'alloc, T> {
    ptr: NonNull<T>,
    length: u32,
    capacity: u32,
    marker: PhantomData<(&'alloc (), T)>,
}

// SAFETY: A list owns its items as a `Box` of a slice would, and holds no reference to its
// allocator, so it takes the auto traits the `Box`'s `Send` impl argues for.
unsafe impl<T: Send> Send for List<'_, T> {}

// SAFETY: Sharing a `&List<T>` shares only a `&[T]`, so it takes `T: Sync`.
unsafe impl<T: Sync> Sync for List<'_, T> {}

impl<'alloc, T> List<'alloc, T> {
    const ASSERT_T_IS_NOT_DROP: () = assert!(
        !mem::needs_drop::<T>(),
        "a `List` is never dropped, so it can't hold a type that needs `Drop`"
    );

    /// A zero-sized item takes no memory, so a list of them never needs to grow.
    const EMPTY_CAPACITY: u32 = if size_of::<T>() == 0 { u32::MAX } else { 0 };

    /// An empty list, which allocates nothing.
    #[must_use]
    pub const fn new() -> Self {
        const { Self::ASSERT_T_IS_NOT_DROP };
        Self {
            ptr: NonNull::dangling(),
            length: 0,
            capacity: Self::EMPTY_CAPACITY,
            marker: PhantomData,
        }
    }

    /// An empty list with room for `capacity` items before a push moves it.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is more than `u32::MAX`.
    pub fn with_capacity_in(capacity: usize, allocator: &'alloc Allocator) -> Self {
        const { Self::ASSERT_T_IS_NOT_DROP };
        if capacity == 0 || size_of::<T>() == 0 {
            return Self::new();
        }
        let capacity = u32::try_from(capacity).expect("a list holds at most `u32::MAX` items");
        let layout = Layout::array::<T>(capacity as usize).expect("the list fits in memory");
        let ptr = allocator.alloc_layout(layout).cast::<T>();
        Self { ptr, length: 0, capacity, marker: PhantomData }
    }

    /// # Panics
    ///
    /// Panics if `items` yields fewer items than its `len()` promised, or more than `u32::MAX`.
    pub fn from_iter_in<I>(items: I, allocator: &'alloc Allocator) -> Self
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut items = items.into_iter();
        let count = items.len();
        if count == 0 {
            return Self::new();
        }
        let layout = Layout::array::<T>(count).expect("the list fits in memory");
        let destination = allocator.alloc_layout(layout).cast::<T>();
        for i in 0..count {
            let item = items.next().expect("the iterator yields as many items as its `len()`");
            // SAFETY: `destination` is a fresh arena allocation with room for `count` items, and
            // `i` is less than `count`.
            let slot = unsafe { destination.add(i) };
            // SAFETY: The slot is allocated and free.
            unsafe { slot.write(item) };
        }
        Self::from_initialized(destination, count)
    }

    /// # Panics
    ///
    /// Panics if `N` is more than `u32::MAX`.
    pub fn from_array_in<const N: usize>(items: [T; N], allocator: &'alloc Allocator) -> Self {
        if N == 0 {
            return Self::new();
        }
        Self::from_initialized(NonNull::from(allocator.alloc(items)).cast::<T>(), N)
    }

    /// Moves `items[at..]` into the arena, leaving `items[..at]` behind, as [`Vec::split_off`]
    /// does.
    ///
    /// # Panics
    ///
    /// Panics if `at > items.len()`, or if more than `u32::MAX` items move.
    pub fn split_off_in(items: &mut Vec<T>, at: usize, allocator: &'alloc Allocator) -> Self {
        let tail = &items[at..];
        if tail.is_empty() {
            return Self::new();
        }
        let count = tail.len();
        let destination = allocator.alloc_layout(Layout::for_value(tail)).cast::<T>();
        // SAFETY: `destination` is a fresh arena allocation with room for `count` items, so it
        // can't overlap `items`.
        unsafe { ptr::copy_nonoverlapping(tail.as_ptr(), destination.as_ptr(), count) };
        // SAFETY: Lowering the length of `items` to `at` hands the copied items over to the list
        // without dropping them.
        unsafe { items.set_len(at) };
        Self::from_initialized(destination, count)
    }

    /// A list of the `length` initialized items at `ptr`, which live in the arena for `'alloc`,
    /// and whose capacity is exactly that.
    fn from_initialized(ptr: NonNull<T>, length: usize) -> Self {
        const { Self::ASSERT_T_IS_NOT_DROP };
        let length = u32::try_from(length).expect("a list holds at most `u32::MAX` items");
        let capacity = if size_of::<T>() == 0 { u32::MAX } else { length };
        Self { ptr, length, capacity, marker: PhantomData }
    }

    /// # Panics
    ///
    /// Panics if the list already holds `u32::MAX` items.
    #[inline]
    pub fn push(&mut self, item: T, allocator: &'alloc Allocator) {
        if self.length == self.capacity {
            self.grow(allocator);
        }
        // SAFETY: The capacity is more than the length, so the slot after the last item is
        // allocated.
        let slot = unsafe { self.ptr.add(self.length as usize) };
        // SAFETY: The slot is allocated and free.
        unsafe { slot.write(item) };
        self.length += 1;
    }

    /// Puts `item` at `index`, shifting every item after it one place later.
    ///
    /// # Panics
    ///
    /// Panics if `index` is past the list's length, or if the list already holds `u32::MAX` items.
    pub fn insert(&mut self, index: usize, item: T, allocator: &'alloc Allocator) {
        let length = self.len();
        assert!(index <= length, "insertion index {index} is past the list's length {length}");
        if self.length == self.capacity {
            self.grow(allocator);
        }
        // SAFETY: `index <= length < capacity`, so the slot at `index` is allocated.
        let slot = unsafe { self.ptr.add(index) };
        // SAFETY: `index + 1 <= capacity`, so the slot after it is allocated, or the end.
        let next_slot = unsafe { slot.add(1) };
        // SAFETY: `length - index` items start at `index`, and the slot after them is allocated
        // and free. `ptr::copy` allows the overlap.
        unsafe { ptr::copy(slot.as_ptr(), next_slot.as_ptr(), length - index) };
        // SAFETY: The slot is allocated, and its item moved one place later.
        unsafe { slot.write(item) };
        self.length += 1;
    }

    /// Moves the items to an arena allocation of twice the capacity, or of 4 items when the list
    /// has none. The old allocation stays in the arena until it's reset.
    #[cold]
    #[inline(never)]
    fn grow(&mut self, allocator: &'alloc Allocator) {
        let capacity = self.capacity.saturating_mul(2).max(4);
        assert!(capacity > self.length, "a list holds at most `u32::MAX` items");
        let layout = Layout::array::<T>(capacity as usize).expect("the list fits in memory");
        let destination = allocator.alloc_layout(layout).cast::<T>();
        #[cfg(feature = "track_allocations")]
        if self.capacity > 0 {
            allocator.counts.update(AllocationCounts::with_reallocation);
        }
        // SAFETY: `destination` is a fresh arena allocation with room for `capacity > length`
        // items, so it can't overlap the `length` initialized items at `ptr`. Those are left
        // behind as copies nothing reads again, which is sound because an item needs no drop.
        unsafe { ptr::copy_nonoverlapping(self.ptr.as_ptr(), destination.as_ptr(), self.len()) };
        self.ptr = destination;
        self.capacity = capacity;
    }

    /// Splits the list in two at `at` without allocating, as [`Vec::split_off`] does: the list
    /// keeps `[..at]`, and the list returned takes `[at..]` and the spare capacity after it, in
    /// place. A push onto the first then moves it to a fresh allocation rather than over the second.
    ///
    /// # Panics
    ///
    /// Panics if `at` is past the list's length.
    #[must_use]
    pub fn split_off(&mut self, at: usize) -> Self {
        let length = self.len();
        assert!(at <= length, "split index {at} is past the list's length {length}");
        let head_length = u32::try_from(at).expect("`at` is at most the length, a `u32`");
        // SAFETY: `at <= length`, so the tail starts in bounds or one past the end.
        let tail_ptr = unsafe { self.ptr.add(at) };
        let is_zero_sized = size_of::<T>() == 0;
        let tail_capacity = if is_zero_sized { u32::MAX } else { self.capacity - head_length };
        let tail = Self {
            ptr: tail_ptr,
            length: self.length - head_length,
            capacity: tail_capacity,
            marker: PhantomData,
        };
        self.length = head_length;
        if !is_zero_sized {
            self.capacity = head_length;
        }
        tail
    }

    /// How many items the list has room for before a push moves it.
    #[must_use]
    #[inline]
    pub const fn capacity(&self) -> usize {
        self.capacity as usize
    }

    /// Keeps the first `new_length` items. A list already that short stays as it is.
    pub fn truncate(&mut self, new_length: usize) {
        if let Ok(new_length) = u32::try_from(new_length)
            && new_length < self.length
        {
            // The items cut off need no drop.
            self.length = new_length;
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

    #[inline]
    fn deref(&self) -> &[T] {
        // SAFETY: The first `length` items at `ptr` are initialized and live in the arena for
        // `'alloc`, and `ptr` is aligned and non-null even when the list is empty.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.length as usize) }
    }
}

impl<T> DerefMut for List<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        // SAFETY: As for `deref`, and the list owns its items, so `&mut self` grants exclusive
        // access to them.
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), self.length as usize) }
    }
}

impl<T: PartialEq> PartialEq for List<'_, T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: Eq> Eq for List<'_, T> {}

impl<T: Hash> Hash for List<'_, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}

impl<T: Debug> Debug for List<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&**self, f)
    }
}

impl<'alloc, T> IntoIterator for List<'alloc, T> {
    type Item = T;
    type IntoIter = ListIntoIter<'alloc, T>;

    fn into_iter(self) -> Self::IntoIter {
        ListIntoIter { list: self, next: 0 }
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

/// A [`List`]'s items, moved out of it in order.
pub struct ListIntoIter<'alloc, T> {
    list: List<'alloc, T>,
    next: u32,
}

impl<T> Iterator for ListIntoIter<'_, T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        if self.next == self.list.length {
            return None;
        }
        // SAFETY: `next < length`, so the slot is allocated.
        let slot = unsafe { self.list.ptr.add(self.next as usize) };
        // SAFETY: The slot holds an initialized item, and it's read only once, since `next` moves
        // past it and the list is never read again otherwise.
        let item = unsafe { slot.read() };
        self.next += 1;
        Some(item)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = (self.list.length - self.next) as usize;
        (remaining, Some(remaining))
    }
}

impl<T> ExactSizeIterator for ListIntoIter<'_, T> {}

/// Leaves the items out, since those already moved out may be borrowed elsewhere.
impl<T> Debug for ListIntoIter<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListIntoIter").finish_non_exhaustive()
    }
}
