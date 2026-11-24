#![expect(
    unsafe_code,
    reason = "an arena hands out raw memory from chunks it links by raw pointers"
)]

mod bump;
mod chunks;

use std::{
    alloc::Layout,
    cell::Cell,
    hint::assert_unchecked,
    mem::{self, MaybeUninit},
    ptr::{self, NonNull},
    slice, str,
};

use self::chunks::ChunkFooter;
#[cfg(feature = "track_allocations")]
use crate::AllocationCounts;

/// A bump arena: it hands out memory from chunks it allocates as it fills them, and frees all of it
/// at once, when it's [`reset`](Self::reset) or dropped.
///
/// It starts with no chunk, and its first allocation makes one big enough for at least that
/// allocation. Each chunk after it is at least twice the size of the one before, and the earlier
/// chunks stay where they are, so a reference into one stays valid, as a reference into a `Vec`
/// doesn't when it grows.
///
/// `reset` keeps only the newest chunk, which is the biggest, and returns the rest to the global
/// allocator. So an allocator reused for similar work settles on a single chunk that holds all of
/// it, which a uniform workload reaches on its third round. From then on no allocation needs a new
/// chunk, so the check for room always branches the same way, and a reset frees nothing. Reusing
/// one also saves the global allocator's calls of making and dropping an allocator, and keeps its
/// memory warm in the cache. An allocator that's never reset grows with everything allocated in
/// it.
///
/// Nothing allocated in it is dropped, which would leak whatever a value owns outside the arena,
/// so every method that stores a value refuses at compile time a type that needs [`Drop`]:
///
/// ```compile_fail
/// use luck_allocator::{Allocator, Box};
///
/// struct Logged;
///
/// impl Drop for Logged {
///     fn drop(&mut self) {}
/// }
///
/// let allocator = Allocator::new();
/// let logged = Box::new_in(Logged, &allocator);
/// ```
///
/// ```compile_fail
/// use luck_allocator::{Allocator, Box};
///
/// struct Owner {
///     items: Vec<u8>,
/// }
///
/// let allocator = Allocator::new();
/// let owner = Box::new_in(Owner { items: vec![1, 2, 3] }, &allocator);
/// ```
///
/// An allocator serves one thread at a time: it's `Send`, but not `Sync`.
//
// `#[repr(C)]` and the field order defeat a store-to-load forwarding hazard on aarch64. The fast
// path reads `cursor` and `start` and writes `cursor` on every allocation. Were the two adjacent,
// at offsets 0 and 8, LLVM's aarch64 backend would fuse their loads into one 16-byte `ldp`, which
// partly overlaps the 8-byte store to `cursor` of the allocation before. That breaks
// store-to-load forwarding, and slows a tight allocation loop about 3 times. `footer` sits between
// them, at offset 8, so they're at offsets 0 and 16, and LLVM emits two 8-byte `ldr`s, each of
// which forwards cleanly. More at
// https://eme64.github.io/blog/2024/06/24/Auto-Vectorization-and-Store-to-Load-Forwarding.html
#[repr(C)]
#[derive(Debug)]
pub struct Allocator {
    /// The start of the newest allocation in the current chunk, or the chunk's footer when it
    /// holds none. An allocation bumps it down toward `start`, and puts its value there.
    ///
    /// INVARIANT: With a chunk, it lies between `start` and the footer, both included, and
    /// without one, it's [`NO_CHUNK`]. It's the current chunk's cursor, whose copy in the footer
    /// is written only when the chunk retires.
    cursor: Cell<NonNull<u8>>,
    /// The current chunk's footer, or `None` when the allocator owns no chunk.
    ///
    /// INVARIANT: It heads the list of every chunk the allocator owns, newest first, each footer
    /// valid for as long as the allocator owns its chunk.
    footer: Cell<Option<NonNull<ChunkFooter>>>,
    /// The start of the current chunk, or [`NO_CHUNK`] without one.
    start: Cell<NonNull<u8>>,
    #[cfg(feature = "track_allocations")]
    pub(crate) counts: Cell<AllocationCounts>,
}

/// The `start` and `cursor` of an allocator that owns no chunk: a dangling pointer, aligned to
/// [`chunks::CHUNK_ALIGN`], for a chunk with no room, so an allocation of a byte or more fails the
/// fast path's bounds check and makes the first chunk on the slow path, with no branch of its own.
const NO_CHUNK: NonNull<u8> = NonNull::<ChunkFooter>::dangling().cast::<u8>();

// SAFETY: Nothing aliases the chunks an allocator owns until it allocates from them, and every
// reference to an allocation borrows the allocator, which keeps it from moving to another thread
// until the borrow ends.
unsafe impl Send for Allocator {}

impl Allocator {
    /// An allocator that owns no memory until its first allocation.
    #[inline]
    pub fn new() -> Self {
        Self {
            cursor: Cell::new(NO_CHUNK),
            footer: Cell::new(None),
            start: Cell::new(NO_CHUNK),
            #[cfg(feature = "track_allocations")]
            counts: Cell::default(),
        }
    }

    /// An allocator whose first chunk holds at least `capacity` bytes, which saves the global
    /// allocator's calls for the chunks it would grow through on its way there. That matters less
    /// for an allocator that's reused.
    ///
    /// # Panics
    ///
    /// Panics if the global allocator can't provide the chunk.
    pub fn with_capacity(capacity: usize) -> Self {
        let allocator = Self::new();
        if capacity == 0 {
            return allocator;
        }
        let layout = Layout::from_size_align(capacity, 1).unwrap_or_else(|_| oom());
        // Sized from `capacity` alone, rather than at least the default size a first allocation
        // gets, so a small capacity takes a small chunk.
        allocator.push_chunk(capacity, layout).unwrap_or_else(|| oom());
        allocator
    }

    /// Moves `value` into the arena.
    ///
    /// # Panics
    ///
    /// Panics if the global allocator can't provide a chunk for it.
    #[expect(clippy::inline_always, reason = "the hottest path in luck, and a few instructions")]
    #[expect(clippy::mut_from_ref, reason = "each allocation is memory nothing else reaches")]
    #[inline(always)]
    pub fn alloc<T>(&self, value: T) -> &mut T {
        const {
            assert!(
                !mem::needs_drop::<T>(),
                "the arena never drops what it holds, so it can't hold a type that needs `Drop`"
            );
        };
        let mut slot = self.alloc_layout(Layout::new::<T>()).cast::<MaybeUninit<T>>();
        // SAFETY: `slot` is fresh memory with `T`'s layout, which `MaybeUninit<T>` shares, so it's
        // aligned and big enough, and nothing else reaches it. It lives as long as `self` is
        // borrowed. A `MaybeUninit` may be uninitialized.
        unsafe { slot.as_mut() }.write(value)
    }

    /// Copies `text` into the arena.
    ///
    /// # Panics
    ///
    /// Panics if the global allocator can't provide a chunk for it.
    #[expect(clippy::inline_always, reason = "a hot path, and a few instructions")]
    #[inline(always)]
    pub fn alloc_str(&self, text: &str) -> &str {
        let bytes = self.alloc_slice_copy(text.as_bytes());
        // SAFETY: The bytes were a `str`'s, so they're UTF-8.
        unsafe { str::from_utf8_unchecked(bytes) }
    }

    /// Copies `items` into the arena.
    ///
    /// # Panics
    ///
    /// Panics if the global allocator can't provide a chunk for them.
    #[expect(clippy::inline_always, reason = "a hot path, and a few instructions")]
    #[expect(clippy::mut_from_ref, reason = "each allocation is memory nothing else reaches")]
    #[inline(always)]
    pub fn alloc_slice_copy<T: Copy>(&self, items: &[T]) -> &mut [T] {
        let destination = self.alloc_layout(Layout::for_value(items)).cast::<T>();
        // SAFETY: `destination` is fresh memory with room for `items`, so it's writable, aligned,
        // and can't overlap them.
        unsafe { ptr::copy_nonoverlapping(items.as_ptr(), destination.as_ptr(), items.len()) };
        // SAFETY: The copy initialized `items.len()` values at `destination`, which nothing else
        // reaches, and which live as long as `self` is borrowed.
        unsafe { NonNull::slice_from_raw_parts(destination, items.len()).as_mut() }
    }

    /// Uninitialized memory for a value of `layout`, which lives until the allocator is reset or
    /// dropped.
    ///
    /// # Panics
    ///
    /// Panics if the global allocator can't provide a chunk for it.
    #[expect(clippy::inline_always, reason = "a hot path, and a few instructions")]
    #[inline(always)]
    pub fn alloc_layout(&self, layout: Layout) -> NonNull<u8> {
        let ptr =
            self.try_alloc_layout_fast(layout).unwrap_or_else(|| self.alloc_layout_slow(layout));
        #[cfg(feature = "track_allocations")]
        self.counts.update(AllocationCounts::with_allocation);
        ptr
    }

    /// `strings` joined into one string in the arena.
    ///
    /// # Panics
    ///
    /// Panics if the joined string would be longer than `isize::MAX` bytes, or if the global
    /// allocator can't provide a chunk for it.
    #[expect(
        clippy::inline_always,
        reason = "the checked additions fold away where some of the strings are known at compile time"
    )]
    #[inline(always)]
    pub fn alloc_concat_strs_array<const N: usize>(&self, strings: [&str; N]) -> &str {
        // A `&str` is at most `isize::MAX` bytes long,
        // https://users.rust-lang.org/t/does-str-reliably-have-length-isize-max/126777, and telling
        // the compiler so lets it drop the overflow checks where the other strings are known, as
        // `[prefix, "require"]`'s are: `isize::MAX + 7` can't overflow a `usize`.
        // https://godbolt.org/z/MGh44Yz5d
        let total_length = strings.iter().fold(0_usize, |total, text| {
            let length = text.len();
            // SAFETY: A `&str` is at most `isize::MAX` bytes long.
            unsafe { assert_unchecked(length <= isize::MAX.unsigned_abs()) };
            total.checked_add(length).expect("the strings' lengths sum to at most `usize::MAX`")
        });
        if total_length == 0 {
            return "";
        }
        assert!(
            isize::try_from(total_length).is_ok(),
            "attempted to create a string longer than `isize::MAX` bytes"
        );
        // SAFETY: `total_length` is the sum of the strings' lengths, and at most `isize::MAX`.
        unsafe { self.concat_strs_with_total_length(strings, total_length) }
    }

    /// `strings` joined in a fresh allocation of `total_length` bytes.
    ///
    /// It's a function of its own so that [`Self::alloc_concat_strs_array`] inlines, for its
    /// checks to fold, while this, a few instructions when some of the strings are known, inlines
    /// as the compiler sees fit.
    ///
    /// # Safety
    ///
    /// `total_length` is the sum of the lengths of `strings`, and at most `isize::MAX`.
    unsafe fn concat_strs_with_total_length<const N: usize>(
        &self,
        strings: [&str; N],
        total_length: usize,
    ) -> &str {
        // SAFETY: The caller guarantees `total_length` is at most `isize::MAX`, and 1 is a power
        // of two.
        let layout = unsafe { Layout::from_size_align_unchecked(total_length, 1) };
        let start = self.alloc_layout(layout);
        let mut end = start;
        for text in strings {
            // SAFETY: `text` is readable for its length. The allocation has room for every string,
            // so `end` has room for this one, and as fresh memory it can't overlap `text`. A byte
            // needs no alignment.
            unsafe { ptr::copy_nonoverlapping(text.as_ptr(), end.as_ptr(), text.len()) };
            // SAFETY: The allocation holds every string, so this lands within it or at its end.
            end = unsafe { end.add(text.len()) };
        }
        debug_assert_eq!(
            end.as_ptr().addr() - start.as_ptr().addr(),
            total_length,
            "the strings fill the allocation"
        );
        // SAFETY: The copies initialized the `total_length` bytes at `start`, which live as long
        // as `self` is borrowed.
        let bytes = unsafe { slice::from_raw_parts(start.as_ptr(), total_length) };
        // SAFETY: Strings joined are UTF-8.
        unsafe { str::from_utf8_unchecked(bytes) }
    }
}

impl Default for Allocator {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

#[inline(never)]
#[cold]
fn oom() -> ! {
    panic!("out of memory")
}
