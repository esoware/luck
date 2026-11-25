use std::{alloc::Layout, cmp::max, hint::assert_unchecked, ptr::NonNull};

use super::{Allocator, MIN_ALIGN, NO_CHUNK};

impl Allocator {
    /// Takes `layout` from the current chunk, by bumping the cursor down past it, or returns
    /// `None` when the chunk has no room for it.
    //
    // The new cursor is the cursor less `layout.size()`, rounded down to a multiple of the
    // larger of `layout.align()` and `MIN_ALIGN`, and it's in the chunk when it's at or after
    // `start`. In what follows, `-` wraps.
    //
    // # Bounds
    //
    // When `layout.align()` is more than `MIN_ALIGN`: a `Layout`'s size rounded up to its
    // alignment is at most `isize::MAX`, so `layout.size()` is at most
    // `isize::MAX - layout.align() + 1`, and rounding down takes at most `layout.align() - 1`
    // more, so the cursor moves down by at most `isize::MAX`. Otherwise the cursor is a multiple
    // of `MIN_ALIGN` already, so it moves down by `layout.size()` rounded up to `MIN_ALIGN`, at
    // most `isize::MAX + 1`.
    //
    // `start` and the cursor are in one allocation, with `start` first, so `cursor - start` is at
    // most `isize::MAX`.
    //
    // When the new cursor is in the chunk, `start <= new <= cursor`, so `new - start` is at most
    // `cursor - start`, and so at most `isize::MAX`.
    //
    // When it's out of the chunk, `new - start` is `(cursor - start) - moved`, which goes below
    // zero and wraps. With `cursor - start` at least 0, and `moved` at most `isize::MAX + 1`, it
    // wraps to at least `usize::MAX + 1 - (isize::MAX + 1)`, which is `isize::MAX + 1`.
    //
    // So `new - start > isize::MAX` exactly when the new cursor is out of the chunk. That tests
    // the top bit, which a `cmp` and a branch on the sign flag do, `js` on x86 and `b.mi` on
    // aarch64. It doesn't assume pointers are in the bottom half of the address space, which a
    // 32-bit target such as WASM breaks.
    //
    // # Shape
    //
    // Telling the compiler the cursor is a multiple of `MIN_ALIGN` lets it work out the rounding
    // of a layout it knows, so a node, whose size is a multiple of 8 and alignment at most 8,
    // takes one subtraction. That holds only while the rounding is written as subtracting the low
    // bits, which known-bits analysis folds, rather than as masking them off.
    //
    // Subtracting the size first, and then rounding down, lets an over-aligned layout, such as
    // size 8 and alignment 16, take only its 8 bytes when the cursor less 8 is already aligned,
    // where rounding the size up first would take 24. A zero-sized allocation needs no case of
    // its own: it moves the cursor only to align it, and leaving out the check keeps the common
    // case fast.
    //
    // Against `bumpalo`'s version, it takes fewer instructions on x86-64 and aarch64, one branch
    // fewer for a layout aligned past a byte, and a single register on x86-64:
    // https://godbolt.org/z/r53d4W943 and https://godbolt.org/z/KKeYnhGjx
    #[expect(clippy::inline_always, reason = "the hottest path in luck, and a few instructions")]
    #[inline(always)]
    pub(super) fn try_alloc_layout_fast(&self, layout: Layout) -> Option<NonNull<u8>> {
        let cursor = self.cursor.get().as_ptr();
        let start = self.start.get().as_ptr();
        debug_assert!(
            start <= cursor,
            "start pointer {start:#p} should be less than or equal to bump pointer {cursor:#p}"
        );
        debug_assert!(
            cursor <= self.footer.get().map_or(NO_CHUNK, NonNull::cast::<u8>).as_ptr(),
            "bump pointer {cursor:#p} should be less than or equal to the footer pointer"
        );
        debug_assert!(
            cursor.addr() - start.addr() <= isize::MAX.unsigned_abs(),
            "distance between start pointer {start:#p} and bump pointer {cursor:#p} should be <= `isize::MAX`"
        );
        debug_assert!(
            cursor.addr().is_multiple_of(MIN_ALIGN),
            "bump pointer {cursor:#p} should be a multiple of `MIN_ALIGN` ({MIN_ALIGN})"
        );
        // SAFETY: The cursor is always a multiple of `MIN_ALIGN`, as the field's invariant says.
        unsafe { assert_unchecked(cursor.addr().is_multiple_of(MIN_ALIGN)) };

        let unaligned = cursor.wrapping_sub(layout.size());
        let align = max(layout.align(), MIN_ALIGN);
        // Rounding the pointer itself down, rather than an integer address, keeps its provenance.
        let new_cursor = unaligned.wrapping_sub(unaligned.addr() & (align - 1));
        if new_cursor.addr().wrapping_sub(start.addr()) > isize::MAX.unsigned_abs() {
            return None;
        }
        debug_assert!(
            new_cursor.addr().is_multiple_of(align),
            "pointer {new_cursor:#p} should be aligned to {align:#}"
        );
        debug_assert!(
            start <= new_cursor && new_cursor <= cursor,
            "pointer {new_cursor:#p} should be in range {start:#p}..{cursor:#p}"
        );
        debug_assert!(!new_cursor.is_null(), "pointer {new_cursor:#p} should not be null");

        // SAFETY: `start` isn't null, so a null `new_cursor` would have wrapped below it and
        // returned above.
        let allocation = unsafe { NonNull::new_unchecked(new_cursor) };
        self.cursor.set(allocation);
        Some(allocation)
    }
}

/// The fast path's bounds check, over allocators whose chunks sit at made-up addresses, including
/// ones in the top half of the address space, which no 64-bit target CI runs on allocates in.
/// Every address derives from `isize::MAX`, so the tests hold on 32-bit targets too.
#[cfg(test)]
mod tests {
    use std::{
        alloc::Layout,
        mem::ManuallyDrop,
        ptr::{self, NonNull},
    };

    use super::super::{MIN_ALIGN, chunks::CHUNK_ALIGN};
    use crate::Allocator;

    /// The first address in the top half of the address space, the one with only its top bit set.
    const TOP: usize = isize::MAX.unsigned_abs() + 1;

    /// An allocator whose current chunk runs from `start` to `cursor`, with its footer at the
    /// cursor. The fast path reads nothing at those addresses, and the allocator is never dropped,
    /// which would free them.
    fn chunk_at(start: usize, cursor: usize) -> ManuallyDrop<Allocator> {
        assert_ne!(start, 0, "start must not be 0");
        assert!(
            start.is_multiple_of(CHUNK_ALIGN),
            "start {start:#x} must be a multiple of `CHUNK_ALIGN` ({CHUNK_ALIGN})"
        );
        assert!(cursor >= start, "cursor {cursor:#x} must be >= start {start:#x}");
        assert!(
            cursor.is_multiple_of(MIN_ALIGN),
            "cursor {cursor:#x} must be a multiple of `MIN_ALIGN` ({MIN_ALIGN})"
        );
        let capacity = cursor - start;
        assert!(
            Layout::from_size_align(capacity, CHUNK_ALIGN).is_ok(),
            "capacity {capacity} with align {CHUNK_ALIGN} must be a valid `Layout`"
        );

        let allocator = ManuallyDrop::new(Allocator::new());
        let cursor_ptr = NonNull::new(ptr::without_provenance_mut(cursor)).unwrap();
        allocator.start.set(NonNull::new(ptr::without_provenance_mut(start)).unwrap());
        allocator.cursor.set(cursor_ptr);
        allocator.footer.set(Some(cursor_ptr.cast()));
        allocator
    }

    fn layout(size: usize, align: usize) -> Layout {
        Layout::from_size_align(size, align).unwrap()
    }

    /// Where the fast path put `layout` in `allocator`, if it took it.
    fn alloc_address(allocator: &Allocator, layout: Layout) -> Option<usize> {
        allocator.try_alloc_layout_fast(layout).map(|ptr| ptr.as_ptr().addr())
    }

    #[test]
    fn bottom_half_enough_room() {
        let allocator = chunk_at(0x1000, 0x2000);
        assert_eq!(alloc_address(&allocator, layout(32, 1)), Some(0x2000 - 32));
    }

    #[test]
    fn bottom_half_exact_fit() {
        let allocator = chunk_at(0x1000, 0x1020);
        assert_eq!(alloc_address(&allocator, layout(32, 1)), Some(0x1000));
    }

    #[test]
    fn bottom_half_not_enough_room() {
        let allocator = chunk_at(0x1000, 0x1010);
        assert_eq!(alloc_address(&allocator, layout(32, 1)), None);
    }

    #[test]
    fn bottom_half_zero_capacity() {
        let allocator = chunk_at(0x1000, 0x1000);
        assert_eq!(alloc_address(&allocator, layout(1, 1)), None);
    }

    #[test]
    fn top_half_enough_room() {
        let start = TOP + 0x1000;
        let cursor = start + 0x1000;
        let allocator = chunk_at(start, cursor);
        assert_eq!(alloc_address(&allocator, layout(32, 1)), Some(cursor - 32));
    }

    #[test]
    fn top_half_exact_fit() {
        let start = TOP + 0x1000;
        let allocator = chunk_at(start, start + 32);
        assert_eq!(alloc_address(&allocator, layout(32, 1)), Some(start));
    }

    #[test]
    fn top_half_not_enough_room() {
        let start = TOP + 0x1000;
        let allocator = chunk_at(start, start + 16);
        assert_eq!(alloc_address(&allocator, layout(32, 1)), None);
    }

    #[test]
    fn top_half_zero_capacity() {
        let start = TOP + 0x1000;
        let allocator = chunk_at(start, start);
        assert_eq!(alloc_address(&allocator, layout(1, 1)), None);
    }

    #[test]
    fn wrapping_sub_wraps_bottom_half() {
        let allocator = chunk_at(0x100, 0x200);
        assert_eq!(alloc_address(&allocator, layout(0x1000, 1)), None);
    }

    /// The cursor is just past the midpoint, and the size takes it below.
    #[test]
    fn wrapping_sub_wraps_across_midpoint() {
        let allocator = chunk_at(TOP, TOP + 0x10);
        assert_eq!(alloc_address(&allocator, layout(0x20, 1)), None);
    }

    #[test]
    fn zst_zero_capacity() {
        let allocator = chunk_at(0x1000, 0x1000);
        assert_eq!(alloc_address(&allocator, layout(0, 1)), Some(0x1000));
    }

    #[test]
    fn zst_top_half() {
        let address = TOP + 0x1000;
        let allocator = chunk_at(address, address);
        assert_eq!(alloc_address(&allocator, layout(0, 1)), Some(address));
    }

    /// 0x1038 less 16 is 0x1028, which rounds down to 0x1020.
    #[test]
    fn aligned_enough_room() {
        let allocator = chunk_at(0x1000, 0x1038);
        assert_eq!(alloc_address(&allocator, layout(16, 16)), Some(0x1020));
    }

    /// 0x1018 less 32 rounds down to 0xFF0, before the start.
    #[test]
    fn aligned_not_enough_room() {
        let allocator = chunk_at(0x1000, 0x1018);
        assert_eq!(alloc_address(&allocator, layout(32, 16)), None);
    }

    /// 0x1018 less 8 is 0x1010, the start, which rounds down to 0x1000, before it.
    #[test]
    fn aligned_rounds_below_start() {
        let allocator = chunk_at(0x1010, 0x1018);
        assert_eq!(alloc_address(&allocator, layout(8, 32)), None);
    }

    #[test]
    fn aligned_top_half() {
        let start = TOP + 0x1000;
        let cursor = start + 0x100;
        let allocator = chunk_at(start, cursor);
        assert_eq!(alloc_address(&allocator, layout(32, 8)), Some(cursor - 32));
    }

    /// `TOP + 0x18` less 8 is `TOP + 0x10`, the start, which rounds down to `TOP`, before it.
    #[test]
    fn aligned_top_half_rounds_below_start() {
        let allocator = chunk_at(TOP + 0x10, TOP + 0x18);
        assert_eq!(alloc_address(&allocator, layout(8, 32)), None);
    }

    #[test]
    fn aligned_top_half_not_enough_room() {
        let start = TOP + 0x1000;
        let allocator = chunk_at(start, start + 16);
        assert_eq!(alloc_address(&allocator, layout(32, 8)), None);
    }

    #[test]
    fn aligned_cursor_already_aligned() {
        let allocator = chunk_at(0x1000, 0x1040);
        assert_eq!(alloc_address(&allocator, layout(16, 8)), Some(0x1040 - 16));
    }

    /// The largest size a byte-aligned `Layout` holds, rounded up to `MIN_ALIGN`, moves the cursor
    /// by `isize::MAX + 1`, the most any layout can, which on a chunk with no room is the worst
    /// case for the bounds check.
    #[test]
    fn max_size() {
        let largest = layout(isize::MAX.unsigned_abs(), 1);
        for (start, cursor) in [
            (0x1000, 0x2000),
            (TOP + 0x1000, TOP + 0x2000),
            (0x1000, 0x1000),
            (TOP + 0x1000, TOP + 0x1000),
        ] {
            let allocator = chunk_at(start, cursor);
            assert_eq!(alloc_address(&allocator, largest), None, "{start:#x}..{cursor:#x}");
        }
    }

    /// `isize::MAX - 7` is the largest size of a valid `Layout` at alignment 8.
    #[test]
    fn max_size_aligned() {
        let largest = layout(isize::MAX.unsigned_abs() - 7, 8);
        for (start, cursor) in [(0x1000, 0x2000), (TOP + 0x1000, TOP + 0x2000)] {
            let allocator = chunk_at(start, cursor);
            assert_eq!(alloc_address(&allocator, largest), None, "{start:#x}..{cursor:#x}");
        }
    }

    /// The largest valid `Layout` at alignment 256, from a cursor 240 past a multiple of 256,
    /// the most a start aligned to `CHUNK_ALIGN` allows, moves the cursor by `isize::MAX - 15`.
    #[test]
    fn boundary_max_aligned_layout_with_max_padding() {
        let largest = layout(isize::MAX.unsigned_abs() - 255, 256);
        for address in [0xF0, TOP + 0xF0] {
            let allocator = chunk_at(address, address);
            assert_eq!(alloc_address(&allocator, largest), None, "{address:#x}");
        }
    }

    // A null new cursor would reach `NonNull::new_unchecked` only past the bounds check, where
    // the debug assertion that it isn't null fails before the undefined behavior, so a test build
    // fails these observably if the check ever lets one through.

    /// 0x1000 less 0x1000 is null.
    #[test]
    fn null_from_size_subtraction() {
        let allocator = chunk_at(CHUNK_ALIGN, 0x1000);
        assert_eq!(alloc_address(&allocator, layout(0x1000, 1)), None);
    }

    /// 0x30 less 4 is 0x2C, which rounds down to null at alignment 64.
    #[test]
    fn null_from_align_rounding() {
        let allocator = chunk_at(CHUNK_ALIGN, 0x30);
        assert_eq!(alloc_address(&allocator, layout(4, 64)), None);
    }

    #[test]
    fn consecutive_allocations() {
        let allocator = chunk_at(0x1000, 0x1100);
        for end in [0x1100 - 32, 0x1100 - 64, 0x1100 - 96] {
            assert_eq!(alloc_address(&allocator, layout(32, 1)), Some(end));
        }
    }

    #[test]
    fn consecutive_allocations_top_half() {
        let start = TOP + 0x1000;
        let cursor = start + 0x100;
        let allocator = chunk_at(start, cursor);
        for end in [cursor - 32, cursor - 64] {
            assert_eq!(alloc_address(&allocator, layout(32, 8)), Some(end));
        }
    }

    #[test]
    fn fill_then_fail() {
        let allocator = chunk_at(0x1000, 0x1040);
        for _ in 0..4 {
            assert!(alloc_address(&allocator, layout(16, 1)).is_some());
        }
        assert_eq!(alloc_address(&allocator, layout(16, 1)), None);
    }

    /// 33 bytes take 40, since the cursor stays a multiple of 8.
    #[test]
    fn off_by_one_too_small() {
        let allocator = chunk_at(0x1000, 0x1020);
        assert_eq!(alloc_address(&allocator, layout(33, 1)), None);
    }

    #[test]
    fn off_by_one_exact() {
        let allocator = chunk_at(0x1000, 0x1020);
        assert_eq!(alloc_address(&allocator, layout(32, 1)), Some(0x1000));
    }

    #[test]
    fn off_by_one_just_enough() {
        let allocator = chunk_at(0x1000, 0x1028);
        assert_eq!(alloc_address(&allocator, layout(33, 1)), Some(0x1000));
    }

    #[test]
    fn off_by_one_top_half() {
        let start = TOP + 0x1000;
        assert_eq!(alloc_address(&chunk_at(start, start + 32), layout(33, 1)), None);
        assert_eq!(alloc_address(&chunk_at(start, start + 32), layout(32, 1)), Some(start));
    }

    /// A chunk can't span the midpoint of a 64-bit target's user space, but can on a 32-bit one.
    #[test]
    fn chunk_spans_midpoint() {
        let cursor = TOP + 0x100;
        let allocator = chunk_at(TOP - 0x100, cursor);
        assert_eq!(alloc_address(&allocator, layout(16, 1)), Some(cursor - 16));
    }

    #[test]
    fn chunk_spans_midpoint_not_enough_room() {
        let allocator = chunk_at(TOP - 0x10, TOP + 0x10);
        assert_eq!(alloc_address(&allocator, layout(64, 1)), None);
    }

    #[test]
    fn near_usize_max() {
        let cursor = usize::MAX - (CHUNK_ALIGN - 1);
        let allocator = chunk_at(cursor - 0x100, cursor);
        assert_eq!(alloc_address(&allocator, layout(16, 1)), Some(cursor - 16));
    }

    #[test]
    fn near_usize_max_not_enough_room() {
        let cursor = usize::MAX - (CHUNK_ALIGN - 1);
        let allocator = chunk_at(cursor - 0x20, cursor);
        assert_eq!(alloc_address(&allocator, layout(64, 1)), None);
    }

    /// A byte takes 8, since the cursor stays a multiple of 8.
    #[test]
    fn size_one_bottom_half() {
        let allocator = chunk_at(0x1000, 0x1008);
        assert_eq!(alloc_address(&allocator, layout(1, 1)), Some(0x1000));
    }

    #[test]
    fn size_one_top_half() {
        let start = TOP + 0x1000;
        let allocator = chunk_at(start, start + 8);
        assert_eq!(alloc_address(&allocator, layout(1, 1)), Some(start));
    }

    /// An alignment below `MIN_ALIGN` rounds the cursor down to a multiple of it all the same, so
    /// a size that isn't one takes the padding up to the next.
    #[test]
    fn small_alignments_keep_the_cursor_a_multiple_of_min_align() {
        let allocator = chunk_at(0x1000, 0x1040);
        for (size, align, address) in
            [(3, 1, 0x1038), (2, 2, 0x1030), (4, 4, 0x1028), (8, 8, 0x1020)]
        {
            assert_eq!(alloc_address(&allocator, layout(size, align)), Some(address), "{size}");
        }
        assert_eq!(alloc_address(&allocator, layout(0, 1)), Some(0x1020));
    }

    /// A zero-sized allocation only rounds the cursor down, here from 0x1038 to 0x1030.
    #[test]
    fn zst_aligned() {
        let allocator = chunk_at(0x1000, 0x1038);
        assert_eq!(alloc_address(&allocator, layout(0, 16)), Some(0x1030));
    }

    #[test]
    fn zst_aligned_exact_start() {
        let allocator = chunk_at(0x1000, 0x1008);
        assert_eq!(alloc_address(&allocator, layout(0, 16)), Some(0x1000));
    }

    #[test]
    fn lowest_valid_start() {
        let cursor = CHUNK_ALIGN + 0x100;
        let allocator = chunk_at(CHUNK_ALIGN, cursor);
        assert_eq!(alloc_address(&allocator, layout(32, 1)), Some(cursor - 32));
    }

    #[test]
    fn lowest_valid_start_not_enough_room() {
        let allocator = chunk_at(CHUNK_ALIGN, CHUNK_ALIGN + 16);
        assert_eq!(alloc_address(&allocator, layout(32, 1)), None);
    }

    /// Size 8 at alignment 16 takes only its 8 bytes when the cursor less 8 is aligned, and pads
    /// to the alignment when it isn't.
    #[test]
    fn over_aligned() {
        let cursor = 0x1000 + 0x1008;
        let allocator = chunk_at(0x1000, cursor);
        for end in [cursor - 8, cursor - 24] {
            assert_eq!(alloc_address(&allocator, layout(8, 16)), Some(end));
        }
    }
}
