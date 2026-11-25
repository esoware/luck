#![expect(
    unsafe_code,
    reason = "a box is a raw pointer into the arena that stands in for a `&mut T`"
)]

use std::{
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem,
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
};

use crate::Allocator;

/// One value in an [`Allocator`]'s arena. Unlike a `std` box it's never dropped, which is why
/// [`Box::new_in`] refuses at compile time a type that needs [`Drop`].
#[repr(transparent)]
pub struct Box<'alloc, T: ?Sized>(NonNull<T>, PhantomData<(&'alloc (), T)>);

// SAFETY: A box has exclusive access to its `T`, and to nothing else, so it takes the auto traits
// of the `&'alloc mut T` it stands in for.
//
// It holds no reference to its allocator, so boxes on two threads can't both allocate from one
// arena: only `new_in` touches an allocator, and it's handed one, so it runs on a thread that
// holds it already. A box is never dropped, so sending one frees no arena memory on another
// thread, and `'alloc` borrows the arena, so it can't be reset or dropped while a box into it
// lives on any thread.
//
// A `T` that isn't `Send` or `Sync` keeps its bound, as a `Box<Cell<T>>` isn't `Sync`, because a
// `Cell` isn't.
unsafe impl<T: Send + ?Sized> Send for Box<'_, T> {}

// SAFETY: Sharing a `&Box<T>` shares only a `&T`, so it takes `T: Sync`, and the `Send` impl
// argues that a box grants access to nothing else.
unsafe impl<T: Sync + ?Sized> Sync for Box<'_, T> {}

impl<T: ?Sized> Box<'_, T> {
    /// Referenced by every method that makes a box.
    const ASSERT_T_IS_NOT_DROP: () = assert!(
        !mem::needs_drop::<T>(),
        "a `Box` is never dropped, so it can't hold a type that needs `Drop`"
    );
}

impl<'alloc, T> Box<'alloc, T> {
    /// Moves `value` into the arena. The box can't outlive the allocator:
    ///
    /// ```compile_fail
    /// use luck_allocator::{Allocator, Box};
    ///
    /// let boxed = {
    ///     let allocator = Allocator::default();
    ///     Box::new_in(5, &allocator)
    /// };
    /// assert_eq!(*boxed, 5);
    /// ```
    #[expect(clippy::inline_always, reason = "a hot path, and a few instructions")]
    #[inline(always)]
    pub fn new_in(value: T, allocator: &'alloc Allocator) -> Self {
        const { Self::ASSERT_T_IS_NOT_DROP };
        Self(NonNull::from(allocator.alloc(value)), PhantomData)
    }

    /// Moves the value back out of the arena.
    #[inline]
    pub fn unbox(self) -> T {
        // SAFETY: A box is only ever made from a fresh allocation or from a pointer its caller
        // guarantees is valid, never from a borrow, so nothing else reads the value, and taking
        // `self` keeps the box from reading it again.
        unsafe { ptr::read(self.0.as_ptr()) }
    }
}

impl<T: ?Sized> Box<'_, T> {
    /// The pointer to the box's value, which isn't valid for writes, and which dangles once the
    /// box is gone.
    #[expect(clippy::inline_always, reason = "a no-op")]
    #[inline(always)]
    pub fn as_non_null(boxed: &Self) -> NonNull<T> {
        boxed.0
    }

    #[expect(clippy::inline_always, reason = "a no-op")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "it consumes the box, as `std`'s `Box::into_non_null` does"
    )]
    #[inline(always)]
    pub fn into_non_null(boxed: Self) -> NonNull<T> {
        boxed.0
    }

    /// # Safety
    ///
    /// `ptr` points to a valid `T` in an [`Allocator`]'s arena that stays valid, and that nothing
    /// else reaches, for as long as the box lives.
    pub const unsafe fn from_non_null(ptr: NonNull<T>) -> Self {
        const { Self::ASSERT_T_IS_NOT_DROP };
        Self(ptr, PhantomData)
    }
}

impl<T> Box<'static, [T]> {
    /// An empty boxed slice, which allocates nothing.
    #[inline]
    pub fn new_empty_boxed_slice() -> Self {
        const { Self::ASSERT_T_IS_NOT_DROP };
        // A dangling pointer is aligned, and alignment is all an empty slice needs to be read:
        // https://doc.rust-lang.org/std/slice/fn.from_raw_parts.html
        Self(NonNull::slice_from_raw_parts(NonNull::dangling(), 0), PhantomData)
    }
}

impl<'alloc, T> Box<'alloc, [T]> {
    /// The boxed slice as a slice that lives as long as the allocator.
    #[expect(clippy::inline_always, reason = "a no-op, as the two share a layout")]
    #[inline(always)]
    pub fn into_arena_slice(self) -> &'alloc [T] {
        // SAFETY: The slice lives until the allocator is reset or dropped, which `'alloc` borrows,
        // and taking `self` leaves nothing with mutable access to it.
        unsafe { self.0.as_ref() }
    }

    /// The boxed slice as a mutable slice that lives as long as the allocator.
    #[expect(clippy::inline_always, reason = "a no-op, as the two share a layout")]
    #[inline(always)]
    pub fn into_arena_slice_mut(mut self) -> &'alloc mut [T] {
        // SAFETY: As for `into_arena_slice`, and taking `self` leaves nothing else with any
        // access to it.
        unsafe { self.0.as_mut() }
    }
}

impl<T: ?Sized> Deref for Box<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        // SAFETY: The pointer is a fresh allocation from `new_in`, an empty slice from
        // `new_empty_boxed_slice`, or one `from_non_null`'s caller guarantees is valid.
        unsafe { self.0.as_ref() }
    }
}

impl<T: ?Sized> DerefMut for Box<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: As for `deref`, and the box has exclusive access to its value.
        unsafe { self.0.as_mut() }
    }
}

impl<T: ?Sized> AsRef<T> for Box<'_, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsMut<T> for Box<'_, T> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized + Display> Display for Box<'_, T> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.deref().fmt(f)
    }
}

impl<T: ?Sized + Debug> Debug for Box<'_, T> {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.deref().fmt(f)
    }
}

impl<T: ?Sized + PartialEq> PartialEq for Box<'_, T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        PartialEq::eq(&**self, &**other)
    }
}

impl<T: ?Sized + Eq> Eq for Box<'_, T> {}

impl<T: ?Sized + Hash> Hash for Box<'_, T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.deref().hash(state);
    }
}
