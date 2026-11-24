use std::{
    cell::Cell,
    hash::{BuildHasher, RandomState},
    ptr::NonNull,
    sync::MutexGuard,
};

use luck_allocator::{Allocator, Box};

use crate::common::{AutoTraits, Fallback as _};

/// A box grants what a `&mut T` does, so it gets the same auto traits.
#[test]
fn auto_traits_follow_the_value_type() {
    const {
        assert!(AutoTraits::<Box<'_, u32>>::IS_SEND);
        assert!(AutoTraits::<Box<'_, u32>>::IS_SYNC);
        // A `Cell` is `Send` but not `Sync`.
        assert!(AutoTraits::<Box<'_, Cell<u32>>>::IS_SEND);
        assert!(!AutoTraits::<Box<'_, Cell<u32>>>::IS_SYNC);
        // A `MutexGuard` is `Sync` but not `Send`.
        assert!(!AutoTraits::<Box<'_, MutexGuard<'_, u32>>>::IS_SEND);
        assert!(AutoTraits::<Box<'_, MutexGuard<'_, u32>>>::IS_SYNC);
    }
}

#[test]
fn unbox_moves_the_value_out() {
    let allocator = Allocator::new();
    assert_eq!(Box::new_in(5, &allocator).unbox(), 5);
}

#[test]
fn deref_mut_writes_through_the_box() {
    let allocator = Allocator::new();
    let mut boxed = Box::new_in("x", &allocator);
    *boxed = allocator.alloc_str("v");
    assert_eq!(*boxed, "v");
}

#[test]
fn new_empty_boxed_slice_is_empty() {
    let empty = Box::<[u32]>::new_empty_boxed_slice();
    assert!(empty.is_empty());
    let nothing: &[u32] = &[];
    assert_eq!(&*empty, nothing);
}

#[test]
#[expect(unsafe_code, reason = "a box made from a pointer is the only kind that holds a slice")]
fn boxed_slice_into_arena_slice() {
    let allocator = Allocator::new();
    let items = allocator.alloc_slice_copy(&[1, 2, 3]);
    // SAFETY: `items` is a slice in `allocator`, which outlives the box, and nothing else
    // reaches it.
    let boxed = unsafe { Box::from_non_null(NonNull::from(items)) };
    assert_eq!(boxed.into_arena_slice(), &[1, 2, 3]);
}

#[test]
#[expect(unsafe_code, reason = "a box made from a pointer is the only kind that holds a slice")]
fn boxed_slice_into_arena_slice_mut() {
    let allocator = Allocator::new();
    let items = allocator.alloc_slice_copy(&[10, 20, 30]);
    // SAFETY: `items` is a slice in `allocator`, which outlives the box, and nothing else
    // reaches it.
    let boxed = unsafe { Box::from_non_null(NonNull::from(items)) };
    let slice = boxed.into_arena_slice_mut();
    slice[1] = 99;
    assert_eq!(slice, &[10, 99, 30]);
}

#[test]
fn compares_hashes_and_prints_by_content() {
    let allocator = Allocator::new();
    let first = Box::new_in("x", &allocator);
    let second = Box::new_in("x", &allocator);
    assert_eq!(first, second);
    assert_ne!(first, Box::new_in("y", &allocator));

    let hasher = RandomState::new();
    assert_eq!(hasher.hash_one(&first), hasher.hash_one(&second));

    assert_eq!(format!("{first:?}"), "\"x\"");
}

#[test]
fn lifetime_variance() {
    fn _shorten<'long: 'short, 'short, T>(boxed: Box<'long, T>) -> Box<'short, T> {
        boxed
    }
}
