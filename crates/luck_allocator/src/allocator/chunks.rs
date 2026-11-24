use std::{
    alloc::{self, Layout},
    cell::Cell,
    cmp::max,
    ptr::NonNull,
};

use super::{Allocator, oom};
#[cfg(feature = "track_allocations")]
use crate::AllocationCounts;

/// The footer at the end of each chunk, which records the chunk's allocation and links to the
/// footer of the chunk before it, so an allocator's chunks form a list from its current chunk back
/// to its first. A chunk allocates from its start up to its footer.
#[repr(C, align(16))]
pub(super) struct ChunkFooter {
    /// The start of the chunk, and of its allocation from the global allocator.
    start: NonNull<u8>,
    /// The layout of the chunk's allocation, footer included.
    layout: Layout,
    /// The footer of the chunk before this one, or `None` for the first.
    previous: Cell<Option<NonNull<ChunkFooter>>>,
    /// The chunk's cursor once it retires, which is stale while it's the current chunk.
    cursor: Cell<NonNull<u8>>,
}

/// The alignment of every chunk and of its footer.
pub(super) const CHUNK_ALIGN: usize = 16;
const CHUNK_FOOTER_SIZE: usize = size_of::<ChunkFooter>();

// `push_chunk`'s arguments that its arithmetic can't overflow rest on these assertions, so a change
// to a constant that breaks one needs a look at it too.
const _: () =
    assert!(align_of::<ChunkFooter>() == CHUNK_ALIGN, "the footer is aligned to `CHUNK_ALIGN`");

/// A typical page size. Chunk sizes needn't match the real one, and it may be smaller, which
/// isn't worth looking up, with the portability concerns and the lost constant propagation that
/// brings.
const TYPICAL_PAGE_SIZE: usize = 0x1000;

const _: () = {
    assert!(TYPICAL_PAGE_SIZE.is_power_of_two(), "the page size is a power of two");
    assert!(TYPICAL_PAGE_SIZE <= (usize::MAX / 8) + 1, "the page size leaves room to add to it");
};

/// The most a typical global allocator spends on each allocation.
const MALLOC_OVERHEAD: usize = 16;

const _: () = {
    assert!(MALLOC_OVERHEAD == CHUNK_ALIGN, "the overhead keeps a chunk aligned");
    assert!(MALLOC_OVERHEAD < TYPICAL_PAGE_SIZE, "the overhead is less than a page");
};

/// What a chunk takes beyond the bytes it can allocate: its footer, the global allocator's
/// overhead, and rounding. A chunk that wants `X` usable bytes, `X` a multiple of `CHUNK_ALIGN`,
/// is expected to take `X + OVERHEAD`, rounded up to the global allocator's next size boundary.
const OVERHEAD: usize = match round_up_to(MALLOC_OVERHEAD + CHUNK_FOOTER_SIZE, CHUNK_ALIGN) {
    Some(overhead) => overhead,
    None => panic!("the overhead fits in a `usize`"),
};

const _: () = {
    assert!(OVERHEAD > CHUNK_FOOTER_SIZE, "the overhead includes the footer");
    // So `OVERHEAD < (usize::MAX / 8) + 1` too.
    assert!(OVERHEAD < TYPICAL_PAGE_SIZE, "the overhead is less than a page");
};

/// The size of the first chunk, overhead included, which oxc chose as one that covers most
/// real-world JavaScript and TypeScript files.
const FIRST_ALLOCATION_GOAL: usize = 16 * 1024;

const _: () = {
    assert!(FIRST_ALLOCATION_GOAL.is_power_of_two(), "the first chunk is a power of two");
    assert!(
        FIRST_ALLOCATION_GOAL <= (usize::MAX / 8) + 1,
        "the first chunk leaves room to add to it"
    );
    assert!(FIRST_ALLOCATION_GOAL > OVERHEAD, "the first chunk holds more than its overhead");
};

/// The usable bytes of the smallest chunk an allocation gets. It's less than
/// `FIRST_ALLOCATION_GOAL` by `OVERHEAD`, so that the whole chunk fits the global allocator's
/// size class rather than spilling into the next one:
/// <https://blog.mozilla.org/nnethercote/2011/08/05/clownshoes-available-in-sizes-2101-and-up/>
const DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER: usize = FIRST_ALLOCATION_GOAL - OVERHEAD;

impl Allocator {
    /// Makes a new chunk the current one, takes `layout` from it, and returns where.
    ///
    /// A new chunk is twice the size of the current one, or of the default size when there's
    /// none, and big enough for `layout` either way. When the global allocator refuses it, the
    /// size halves until it would be too small for `layout` or below the default.
    #[inline(never)]
    #[cold]
    pub(super) fn alloc_layout_slow(&self, layout: Layout) -> NonNull<u8> {
        let min_size = max(layout.size(), DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER);
        let mut size = match self.footer.get() {
            Some(footer) => {
                // SAFETY: The current footer is valid while the allocator owns its chunk.
                let current_size = unsafe { footer.as_ref() }.layout.size() - CHUNK_FOOTER_SIZE;
                // A `Layout`'s size is at most `isize::MAX`, so doubling it can't overflow.
                max(current_size * 2, min_size)
            }
            None => min_size,
        };
        while self.push_chunk(size, layout).is_none() {
            size /= 2;
            if size < min_size {
                oom();
            }
        }
        self.try_alloc_layout_fast(layout).unwrap_or_else(|| oom())
    }

    /// Makes a fresh chunk the current one, with room for `requested_layout` and for at least
    /// `min_size_without_footer` bytes. The size rounds up to suit the global allocator, which wastes
    /// least: for a small chunk, to a power of two with the footer and the global allocator's
    /// overhead included, and for a larger one, to a page boundary.
    ///
    /// Returns `None`, and leaves the allocator as it was, when the size is too large for a
    /// `Layout` or the global allocator refuses it.
    pub(super) fn push_chunk(
        &self,
        min_size_without_footer: usize,
        requested_layout: Layout,
    ) -> Option<()> {
        // The footer sits a multiple of `CHUNK_ALIGN` past the start, so the chunk takes its
        // alignment, or the layout's when that's more.
        let align = max(requested_layout.align(), CHUNK_ALIGN);
        // SAFETY: A `Layout`'s size is at most `isize::MAX`, and `align` is a power of two, so
        // it rounds up to at most `isize::MAX + 1`, which fits in a `usize`.
        let requested_size = unsafe { round_up_to_unchecked(requested_layout.size(), align) };
        let wanted_size_without_footer = max(min_size_without_footer, requested_size);

        let size_with_overhead = if wanted_size_without_footer < TYPICAL_PAGE_SIZE {
            // `TYPICAL_PAGE_SIZE` and `OVERHEAD` are both at most `(usize::MAX / 8) + 1`, and
            // the wanted size is under `TYPICAL_PAGE_SIZE`, so the sum is at most
            // `usize::MAX / 4`, and its next power of two at most `(usize::MAX / 4) + 1`, neither
            // of which overflows. The result is at least `OVERHEAD` and under `isize::MAX`, so the
            // chunk's size is a valid `Layout` size for any alignment but `isize::MAX + 1`.
            (wanted_size_without_footer + OVERHEAD).next_power_of_two()
        } else {
            // Nothing bounds the wanted size here, so the largest size that could still
            // make a valid `Layout` at `CHUNK_ALIGN`, the smallest alignment, rules out the sizes
            // that would overflow below. It doesn't promise a valid `Layout` at every alignment,
            // but it rules out only sizes that would fail later anyway.
            const MAX_SIZE_WITHOUT_FOOTER: usize = isize::MAX.unsigned_abs() + 1 - OVERHEAD;
            if wanted_size_without_footer > MAX_SIZE_WITHOUT_FOOTER {
                return None;
            }
            // SAFETY: The check above keeps the wanted size plus `OVERHEAD` from overflowing,
            // and rounding it up to a page from passing `isize::MAX + 1`. So the result is at
            // least `OVERHEAD`, and the chunk's size is a valid `Layout` size at `CHUNK_ALIGN`,
            // though maybe not at a larger alignment.
            unsafe {
                round_up_to_unchecked(wanted_size_without_footer + OVERHEAD, TYPICAL_PAGE_SIZE)
            }
        };
        debug_assert!(
            size_with_overhead <= isize::MAX.unsigned_abs() + 1,
            "the chunk's size rounds to at most `isize::MAX + 1`"
        );

        // `size_with_overhead` is at least `OVERHEAD`, which is more than `CHUNK_FOOTER_SIZE`, so
        // taking off the one and adding back the other overflows neither way.
        let size_without_footer = size_with_overhead - OVERHEAD;
        let size = size_without_footer + CHUNK_FOOTER_SIZE;
        debug_assert!(
            align.is_multiple_of(CHUNK_ALIGN) && size_without_footer.is_multiple_of(CHUNK_ALIGN),
            "the footer is aligned"
        );
        let layout = Layout::from_size_align(size, align).ok()?;
        debug_assert!(size >= requested_layout.size(), "the chunk has room for the allocation");

        // Marks the allocation as a chunk's for a global allocator that counts calls, so it must
        // come right before it, with no allocation between.
        #[cfg(feature = "track_allocations")]
        crate::tracking::start_chunk_operation();
        // SAFETY: `layout`'s size includes the footer, so it isn't zero.
        let start = NonNull::new(unsafe { alloc::alloc(layout) })?;
        // SAFETY: The allocation is `size_without_footer + CHUNK_FOOTER_SIZE` bytes from `start`,
        // so the footer's place is within it.
        let footer = unsafe { start.add(size_without_footer) }.cast::<ChunkFooter>();
        debug_assert!(
            start.addr().get().is_multiple_of(align)
                && footer.addr().get().is_multiple_of(CHUNK_ALIGN),
            "the chunk and its footer are aligned"
        );

        let previous = self.footer.get();
        // A chunk allocates down from its footer, which `CHUNK_ALIGN` aligns.
        let cursor = footer.cast::<u8>();
        // SAFETY: The footer's place is aligned for a `ChunkFooter` and ends the allocation.
        unsafe {
            footer.write(ChunkFooter {
                start,
                layout,
                previous: Cell::new(previous),
                cursor: Cell::new(cursor),
            });
        }
        if let Some(retiring) = previous {
            // A retiring chunk keeps its cursor in its footer, where `used_bytes` reads it.
            // SAFETY: The current footer is valid while the allocator owns its chunk.
            unsafe { retiring.as_ref() }.cursor.set(self.cursor.get());
        }
        self.start.set(start);
        self.cursor.set(cursor);
        self.footer.set(Some(footer));
        Some(())
    }

    /// Frees everything allocated in the allocator at once, without dropping any of it, and
    /// returns every chunk but the newest, which is the biggest, to the global allocator, keeping
    /// that one for what's allocated next.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "taking `&mut self` is what proves nothing borrows from the arena it frees"
    )]
    pub fn reset(&mut self) {
        #[cfg(feature = "track_allocations")]
        self.counts.set(AllocationCounts::default());
        let Some(footer) = self.footer.get() else {
            return;
        };
        // SAFETY: The current footer is valid while the allocator owns its chunk.
        let retired = unsafe { footer.as_ref() }.previous.replace(None);
        // SAFETY: `&mut self` means nothing borrows from the arena, and unlinking the retired
        // chunks leaves nothing that reaches them.
        unsafe { dealloc_chunks(retired) };
        // The footer's own copy of the cursor is stale until the chunk retires.
        self.cursor.set(footer.cast::<u8>());
    }

    /// The bytes the allocator's chunks can hold, whether used or not, which leaves out their
    /// footers, so the allocator takes more than this from the global allocator. See
    /// [`used_bytes`](Self::used_bytes) for what's allocated in them.
    pub fn capacity(&self) -> usize {
        let mut total = 0;
        let mut next = self.footer.get();
        while let Some(footer) = next {
            // SAFETY: Every footer in the list is valid while the allocator owns its chunk.
            let chunk = unsafe { footer.as_ref() };
            total += chunk.layout.size() - CHUNK_FOOTER_SIZE;
            next = chunk.previous.get();
        }
        total
    }

    /// The bytes allocated in the allocator since it was made or last reset. They include the
    /// padding that aligns each allocation, and every allocation still in the arena whether or
    /// not anything reaches it, such as the storage a `List` left behind when it grew, since the
    /// arena reuses nothing. So it's almost always more than the live data, though a tree cloned
    /// into a new allocator, in a different order, may need slightly more for padding.
    pub fn used_bytes(&self) -> usize {
        let Some(current) = self.footer.get() else {
            return 0;
        };
        // The current chunk's cursor is the allocator's, and a retired chunk's is in its footer.
        // SAFETY: The cursor lies in the current chunk, at or before its footer.
        let mut total = unsafe { current.cast::<u8>().offset_from_unsigned(self.cursor.get()) };
        // SAFETY: The current footer is valid while the allocator owns its chunk.
        let mut next = unsafe { current.as_ref() }.previous.get();
        while let Some(footer) = next {
            // SAFETY: Every footer in the list is valid while the allocator owns its chunk.
            let retired = unsafe { footer.as_ref() };
            let cursor = retired.cursor.get();
            debug_assert!(cursor <= footer.cast::<u8>(), "the cursor is in its chunk");
            // SAFETY: A retired chunk's cursor lies in it, at or before its footer.
            total += unsafe { footer.cast::<u8>().offset_from_unsigned(cursor) };
            next = retired.previous.get();
        }
        total
    }
}

impl Drop for Allocator {
    fn drop(&mut self) {
        // SAFETY: Dropping the allocator ends every borrow from the arena, and nothing reaches
        // its chunks after.
        unsafe { dealloc_chunks(self.footer.get()) };
    }
}

/// Returns `newest`'s chunk, and every chunk before it, to the global allocator.
///
/// # Safety
///
/// `newest` is `None`, or heads a list of an allocator's chunks that nothing borrows from or
/// reaches again.
#[inline]
unsafe fn dealloc_chunks(newest: Option<NonNull<ChunkFooter>>) {
    let mut next = newest;
    while let Some(footer) = next {
        // The footer is part of the chunk, so the reference to it ends before the chunk goes.
        let (start, layout) = {
            // SAFETY: Every footer in the list is valid until its chunk goes, below.
            let chunk = unsafe { footer.as_ref() };
            next = chunk.previous.get();
            (chunk.start, chunk.layout)
        };
        // Marks the deallocation as a chunk's for a global allocator that counts calls, so it
        // must come right before it, with no allocation between.
        #[cfg(feature = "track_allocations")]
        crate::tracking::start_chunk_operation();
        // SAFETY: `start` and `layout` describe the chunk's allocation from the global allocator.
        unsafe { alloc::dealloc(start.as_ptr(), layout) };
    }
}

/// `n` rounded up to a multiple of `divisor`, a power of two, or `None` when that overflows.
#[inline]
const fn round_up_to(n: usize, divisor: usize) -> Option<usize> {
    debug_assert!(divisor.is_power_of_two(), "the divisor is a power of two");
    match n.checked_add(divisor - 1) {
        Some(sum) => Some(sum & !(divisor - 1)),
        None => None,
    }
}

/// [`round_up_to`] for a result known to fit, which lets the compiler drop the check.
///
/// # Safety
///
/// `n` rounded up to a multiple of `divisor` fits in a `usize`.
#[inline]
unsafe fn round_up_to_unchecked(n: usize, divisor: usize) -> usize {
    // SAFETY: The caller guarantees the rounding doesn't overflow.
    unsafe { round_up_to(n, divisor).unwrap_unchecked() }
}

#[cfg(test)]
mod tests {
    use std::{alloc::Layout, iter, ptr};

    use super::{DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER, OVERHEAD, TYPICAL_PAGE_SIZE};
    use crate::Allocator;

    fn chunk_count(allocator: &Allocator) -> usize {
        iter::successors(allocator.footer.get(), |footer| {
            // SAFETY: Every footer in the list is valid while the allocator owns its chunk.
            unsafe { footer.as_ref() }.previous.get()
        })
        .count()
    }

    #[test]
    fn capacity_and_used_bytes_follow_each_chunk() {
        // A new chunk has twice the usable bytes of the one before, rounded up to a page with
        // `OVERHEAD` included, as `push_chunk` sizes it.
        const fn next_chunk_size(previous: usize) -> usize {
            (2 * previous + OVERHEAD).next_multiple_of(TYPICAL_PAGE_SIZE) - OVERHEAD
        }
        const SECOND_CHUNK: usize = next_chunk_size(DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER);
        const THIRD_CHUNK: usize = next_chunk_size(SECOND_CHUNK);

        let mut allocator = Allocator::new();
        assert_eq!(allocator.capacity(), 0);
        assert_eq!(allocator.used_bytes(), 0);
        assert_eq!(chunk_count(&allocator), 0);

        allocator.alloc(0_u8);
        assert_eq!(allocator.capacity(), DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER);
        assert_eq!(allocator.used_bytes(), 1);
        assert_eq!(chunk_count(&allocator), 1);

        // A reset keeps the only chunk, and empties it.
        allocator.reset();
        assert_eq!(allocator.capacity(), DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER);
        assert_eq!(allocator.used_bytes(), 0);
        assert_eq!(chunk_count(&allocator), 1);

        let small = Layout::from_size_align(64, 1).unwrap();
        allocator.alloc_layout(small);
        allocator.alloc_layout(small);
        assert_eq!(allocator.capacity(), DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER);
        assert_eq!(allocator.used_bytes(), small.size() * 2);
        assert_eq!(chunk_count(&allocator), 1);

        // The first chunk retires with the two small allocations, and the second holds the big
        // one.
        let big = Layout::from_size_align(DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER, 1).unwrap();
        allocator.alloc_layout(big);
        assert_eq!(allocator.capacity(), DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER + SECOND_CHUNK);
        assert_eq!(allocator.used_bytes(), small.size() * 2 + big.size());
        assert_eq!(chunk_count(&allocator), 2);

        // Bigger than any chunk so far, where another `big` might fit beside the first.
        let huge = Layout::from_size_align(DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER * 2, 1).unwrap();
        allocator.alloc_layout(huge);
        assert_eq!(
            allocator.capacity(),
            DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER + SECOND_CHUNK + THIRD_CHUNK
        );
        assert_eq!(allocator.used_bytes(), small.size() * 2 + big.size() + huge.size());
        assert_eq!(chunk_count(&allocator), 3);

        // A reset keeps only the newest chunk, which is the biggest.
        allocator.reset();
        assert_eq!(allocator.capacity(), THIRD_CHUNK);
        assert_eq!(allocator.used_bytes(), 0);
        assert_eq!(chunk_count(&allocator), 1);
    }

    #[test]
    fn reset_allocates_again_from_the_end_of_the_newest_chunk() {
        let mut allocator = Allocator::new();
        for value in 0_u64..10_000 {
            allocator.alloc(value);
        }
        assert!(chunk_count(&allocator) > 1);
        let newest = allocator.footer.get().unwrap();
        allocator.reset();
        assert_eq!(chunk_count(&allocator), 1);
        assert_eq!(allocator.footer.get(), Some(newest));
        let first = ptr::from_mut(allocator.alloc(0_u64)).addr();
        assert_eq!(first, newest.addr().get() - size_of::<u64>());
    }
}
