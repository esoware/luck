//! Main implementation of allocation methods.
//!
//! Public allocation methods are in `alloc.rs`.
//! They all ultimately call into `alloc_layout`, defined in this module.
//! (`alloc_layout` is also a public method)

use std::{alloc::Layout, cmp::max, hint::assert_unchecked, ptr::NonNull};

use super::{
    Arena, CHUNK_FOOTER_SIZE, DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER, EMPTY_ARENA_DATA_PTR,
    utils::{is_pointer_aligned_to, oom, round_mut_ptr_down_to},
};

impl<const MIN_ALIGN: usize> Arena<MIN_ALIGN> {
    /// Allocate space for an object with the given `Layout`.
    ///
    /// The returned pointer points at uninitialized memory, and should be initialized with [`std::ptr::write`].
    ///
    /// # Panics
    ///
    /// Panics if reserving space matching `layout` fails.
    #[inline(always)]
    pub fn alloc_layout(&self, layout: Layout) -> NonNull<u8> {
        self.try_alloc_layout_fast(layout).unwrap_or_else(|| self.alloc_layout_slow(layout))
    }

    // Only `pub(super)` to expose it for unit tests
    #[inline(always)]
    pub(super) fn try_alloc_layout_fast(&self, layout: Layout) -> Option<NonNull<u8>> {
        let cursor_ptr = self.cursor_ptr.get().as_ptr();
        let start_ptr = self.start_ptr.get().as_ptr();

        debug_assert!(
            start_ptr <= cursor_ptr,
            "start pointer {start_ptr:#p} should be less than or equal to bump pointer {cursor_ptr:#p}"
        );
        if cfg!(debug_assertions) {
            let end_ptr = self
                .current_chunk_footer_ptr
                .get()
                .map_or(EMPTY_ARENA_DATA_PTR, NonNull::cast::<u8>);
            assert!(
                cursor_ptr <= end_ptr.as_ptr(),
                "bump pointer {cursor_ptr:#p} should be less than or equal to footer pointer {end_ptr:#p}",
            );
        }
        #[expect(clippy::checked_conversions)]
        {
            debug_assert!(
                cursor_ptr.addr() - start_ptr.addr() <= isize::MAX as usize,
                "distance between start pointer {start_ptr:#p} and bump pointer {cursor_ptr:#p} should be <= isize::MAX"
            );
        }
        debug_assert!(
            is_pointer_aligned_to(cursor_ptr, MIN_ALIGN),
            "bump pointer {cursor_ptr:#p} should be aligned to the minimum alignment of {MIN_ALIGN:#x}"
        );

        // Compute `new_ptr` by:
        //
        // 1. Subtract `layout.size()` from `cursor_ptr`.
        // 2. Round the result down to be aligned for both `MIN_ALIGN` and `layout.align()`.
        // 3. Check if pointer is within bounds of current chunk.
        //
        // ------------
        // # Invariants
        // ------------
        //
        // This implementation relies on the following invariants:
        //
        // 1. `layout.size()` is always `<= isize::MAX`.
        // 2. `layout.size()`, when rounded up to the nearest multiple of `layout.align()`, is always `<= isize::MAX`.
        // 3. `cursor_ptr >= start_ptr`, because `cursor_ptr` is within the current chunk, which starts at `start_ptr`.
        // 4. `cursor_ptr - start_ptr` is always `<= isize::MAX`, because they are both within the same allocation.
        // 5. `MIN_ALIGN` is a power of two.
        // 6. `cursor_ptr` is always aligned to `MIN_ALIGN`.
        //
        // Invariants 1 and 2 are invariants of `Layout` type.
        // Invariant 4 is guaranteed by Rust.
        // Invariants 3, 5 and 6 are guaranteed by `Arena` type.
        //
        // ---------------------
        // # Computing `new_ptr`
        // ---------------------
        //
        // ## `layout.align() > MIN_ALIGN`
        //
        // Subtract `layout.size()` from `cursor_ptr`, then round down to a multiple of `layout.align()` => `new_ptr`.
        //
        // `Layout`'s invariant is that `layout.size()`, when rounded up to the nearest multiple of `layout.align()`,
        // is always `<= isize::MAX`.
        // So the maximum subtraction (before rounding) is `isize::MAX - align + 1`.
        //
        // Rounding down to `align` can subtract at most a further `align - 1`.
        // Therefore, maximum total subtraction including rounding is `isize::MAX - align + 1 + align - 1`
        // = `isize::MAX`.
        //
        // ## `layout.align() <= MIN_ALIGN`
        //
        // Subtract `layout.size()` from `cursor_ptr`, then round down to a multiple of `MIN_ALIGN` => `new_ptr`.
        //
        // `Layout`'s invariant is that `layout.size() <= isize::MAX`.
        // So the maximum subtraction (before rounding) is `isize::MAX`.
        //
        // Rounding down to `MIN_ALIGN` can subtract at most a further `MIN_ALIGN - 1`.
        // However, since `cursor_ptr` is itself aligned to `MIN_ALIGN` (`Arena` invariant),
        // the total subtraction (size + rounding padding) equals `round_up(layout.size(), MIN_ALIGN)`.
        //
        // `MIN_ALIGN` is a power of 2 and a `usize`, so at most `isize::MAX + 1`.
        // `layout.size() <= isize::MAX` (invariant of `Layout`).
        // So the maximum value of `round_up(layout.size(), MIN_ALIGN)` is `round_up(isize::MAX, isize::MAX + 1)`,
        // which is `isize::MAX + 1`.
        //
        // Therefore maximum total subtraction including rounding is `isize::MAX + 1`.
        //
        // ## Combined
        //
        // Considering both cases, the maximum total subtraction is `isize::MAX + 1`.
        //
        // -------------------------
        // # Detecting out of bounds
        // -------------------------
        //
        // ## In bounds
        //
        // When `new_ptr` is within current chunk:
        //   * `start_ptr <= new_ptr <= cursor_ptr`.
        //   * Invariant: `cursor_ptr - start_ptr <= isize::MAX`.
        //   * So `new_ptr - start_ptr` does not wrap and is `<= isize::MAX`.
        //   * Therefore `new_ptr.wrapping_sub(start_ptr) <= isize::MAX`.
        //
        // ## Out of bounds
        //
        // When `new_ptr` is outside current chunk:
        //
        // (in all calculations that follow, "-" means wrapping subtraction)
        //
        // * `new_ptr = cursor_ptr - total_subtraction`.
        // * `cursor_ptr = start_ptr + (cursor_ptr - start_ptr)` (trivially true).
        // * So `new_ptr = start_ptr + (cursor_ptr - start_ptr) - total_subtraction`.
        // * So `new_ptr - start_ptr = start_ptr + (cursor_ptr - start_ptr) - total_subtraction - start_ptr`
        //    => `new_ptr - start_ptr = (cursor_ptr - start_ptr) - total_subtraction`
        // * Invariant: `cursor_ptr - start_ptr <= isize::MAX`.
        // * `total_subtraction` is `<= isize::MAX + 1` (proved above).
        // * `new_ptr.wrapping_sub(start_ptr)` always wraps (if `new_ptr >= start_ptr`, it would be in bounds).
        // * In most extreme case:
        //   * `cursor_ptr == start_ptr` (chunk has no empty space).
        //   * `total_subtraction == isize::MAX + 1` (its maximum).
        //   * `new_ptr = cursor_ptr.wrapping_sub(isize::MAX + 1)`.
        //   * `new_ptr = start_ptr.wrapping_sub(isize::MAX + 1)`.
        //   * `new_ptr.wrapping_sub(start_ptr) = start_ptr.wrapping_sub(isize::MAX + 1).wrapping_sub(start_ptr)`
        //     = `0.wrapping_sub(isize::MAX + 1)`
        //     = `usize::MAX + 1 - (isize::MAX + 1)`
        //     = `isize::MAX + 1`
        // * In all other out-of-bounds cases:
        //   * `cursor_ptr - start_ptr` is larger, or `total_subtraction` is smaller (or both).
        //   * Either change makes the wrapping subtraction wrap less (result is closer to `usize::MAX`).
        //   * So `new_ptr.wrapping_sub(start_ptr) > isize::MAX + 1`.
        // * Therefore, in all out of bounds cases, `new_ptr.wrapping_sub(start_ptr) > isize::MAX`.
        //
        // ## Detection
        //
        // The difference between in bounds / out of bounds is:
        // * In bounds:     `new_ptr.wrapping_sub(start_ptr) <= isize::MAX`.
        // * Out of bounds: `new_ptr.wrapping_sub(start_ptr) > isize::MAX`.
        //
        // Therefore `new_ptr.wrapping_sub(start_ptr) > isize::MAX` separates "in bounds" from "out of bounds".
        //
        // `> isize::MAX` is equivalent to checking if the top bit is set, which is very efficient in assembly.
        // * `cmp new_ptr, start_ptr` subtracts and sets the sign flag (`SF` on x86, `N` on aarch64).
        // * Branch on the sign flag (`js` on x86, `b.mi` on aarch64).
        //
        // Note: This calculation does *not* rely on pointers always being in bottom half of address space,
        // which cannot be relied on, in particular on 32-bit platforms e.g. WASM.
        //
        // ---------------
        // # Const folding
        // ---------------
        //
        // `MIN_ALIGN` is a constant, and `layout.align()` is also constant in practice after inlining,
        // so `max(layout.align(), MIN_ALIGN)` is folded down to a constant integer.
        //
        // For statically-known `layout.size()` (e.g. in `Arena::alloc`):
        // We inform compiler that `cursor_ptr` is always aligned to `MIN_ALIGN` with an unchecked assertion.
        // Combined with `round_mut_ptr_down_to`'s implementation as `ptr.wrapping_sub(ptr.addr() & (divisor - 1))`,
        // LLVM's known-bits analysis can:
        //
        // * compute the `& (divisor - 1)` term as a constant, and
        // * fold `(cursor - constant_size) - constant_low_bits` into a single `cursor - constant`.
        //
        // Note: Writing the rounding as `p & !(divisor - 1)` instead of `p - (p & (divisor - 1))`
        // would defeat this optimization — LLVM doesn't recognize the equivalent fold for the AND form.
        //
        // ---------------------------------------------------
        // # Why not round up size first, then subtract after?
        // ---------------------------------------------------
        //
        // For statically-known `layout.size()` (e.g. in `Arena::alloc`), due to const-folding,
        // either version produces the same number of instructions.
        //
        // Subtracting first, then round down is preferable in 2 cases:
        //
        // 1. Dynamic size when `align < MIN_ALIGN`. e.g. `alloc_str` in an `Arena<MIN_ALIGN = 8>`:
        //    Rounding down is 1 instruction, whereas rounding up is 2 instructions.
        //    So the current version (subtract then round down) is 1 instruction shorter.
        //
        // 2. Over-aligned layouts e.g. size 8, align 16.
        //    Current version (subtract then round down) will only consume 8 bytes of arena memory if cursor
        //    is already positioned so that `cursor_ptr - 8` is aligned on 16.
        //    In comparison, rounding up size to multiple of 16 first, then subtracting, then rounding pointer down
        //    would consume 24 bytes.
        //
        // ------------------------
        // # Zero-sized allocations
        // ------------------------
        //
        // Zero-sized allocations require no special handling.
        // The pointer will be bumped by zero bytes, modulo alignment.
        // Avoiding a `layout.size() == 0` check keeps this code optimized for non-ZSTs, which are much more common.
        //
        // --------------
        // # Instructions
        // --------------
        //
        // This formulation overall produces the minimum possible instruction count for all use cases.
        //
        // Compared to `bumpalo`'s original implementation:
        // * Fewer instructions on both x86-64 and aarch64.
        // * When `layout.align() > MIN_ALIGN`, additionally removes a branch.
        // * On x86-64, register usage reduced to only 1 register.
        //
        // x86: https://godbolt.org/z/r53d4W943
        // aarch64: https://godbolt.org/z/KKeYnhGjx

        // SAFETY: `cursor_ptr` is always aligned to `MIN_ALIGN` (invariant of `Arena`)
        unsafe { assert_unchecked(cursor_ptr.addr().is_multiple_of(MIN_ALIGN)) };

        let new_ptr = cursor_ptr.wrapping_sub(layout.size());

        let align = max(layout.align(), MIN_ALIGN);
        let new_ptr = round_mut_ptr_down_to(new_ptr, align);

        if new_ptr.addr().wrapping_sub(start_ptr.addr()) > isize::MAX as usize {
            return None;
        }

        debug_assert!(
            is_pointer_aligned_to(new_ptr, layout.align()),
            "pointer {new_ptr:#p} should be aligned to layout alignment of {:#}",
            layout.align()
        );
        debug_assert!(
            is_pointer_aligned_to(new_ptr, MIN_ALIGN),
            "pointer {new_ptr:#p} should be aligned to minimum alignment of {MIN_ALIGN:#}"
        );
        debug_assert!(
            start_ptr <= new_ptr && new_ptr <= cursor_ptr,
            "pointer {new_ptr:#p} should be in range {start_ptr:#p}..{cursor_ptr:#p}"
        );
        debug_assert!(!new_ptr.is_null());

        // SAFETY: `start_ptr` is non-null, so if `new_ptr` was null, `new_ptr.addr().wrapping_sub(start_ptr.addr())`
        // above would have wrapped around, and we already exited
        let new_ptr = unsafe { NonNull::new_unchecked(new_ptr) };

        // Update cursor
        self.cursor_ptr.set(new_ptr);

        // Return the pointer
        Some(new_ptr)
    }

    /// Slow path for [`Arena::alloc_layout`].
    ///
    /// Called when there isn't enough room in our current chunk, so need to allocate a new chunk.
    #[inline(never)]
    #[cold]
    fn alloc_layout_slow(&self, layout: Layout) -> NonNull<u8> {
        self.try_alloc_layout_slow_impl(layout).unwrap_or_else(|| oom())
    }

    /// Slow path for allocation.
    /// Called when there isn't enough room in our current chunk, so need to allocate a new chunk.
    fn try_alloc_layout_slow_impl(&self, layout: Layout) -> Option<NonNull<u8>> {
        let current_footer_ptr = self.current_chunk_footer_ptr.get();

        // Get a new chunk from the global allocator.
        // By default, we want our new chunk to be about twice as big as the previous chunk.
        // If the global allocator refuses it, we try to divide it by half until it works for `layout`
        // or the requested size is smaller than the default chunk size.
        let min_new_chunk_size = max(layout.size(), DEFAULT_CHUNK_SIZE_WITHOUT_FOOTER);

        let mut size = match current_footer_ptr {
            // Double the size of the current chunk, but not less than `min_new_chunk_size`
            Some(footer_ptr) => {
                // SAFETY: `footer_ptr` always points to a valid `ChunkFooter`
                let footer = unsafe { footer_ptr.as_ref() };
                let current_size_without_footer = footer.layout.size() - CHUNK_FOOTER_SIZE;
                // `current_size_without_footer * 2` cannot overflow because `Layout::size()` is always `<= isize::MAX`
                max(current_size_without_footer * 2, min_new_chunk_size)
            }
            // No existing chunks, so just use `min_new_chunk_size`
            None => min_new_chunk_size,
        };

        let (new_start_ptr, new_footer_ptr) = loop {
            // Try to allocate a chunk of `size` bytes (plus footer and rounding).
            // SAFETY: `current_footer_ptr` points to a valid `ChunkFooter` for a chunk in this `Arena`, or it's `None`.
            let new_chunk = unsafe { Self::new_chunk(size, layout, current_footer_ptr) };
            if let Some(new_chunk) = new_chunk {
                break new_chunk;
            }

            // Failed. Halve size and try again.
            // TODO: Before admitting defeat, try one last time with minimum size required to service the request.
            // This can be a value somewhere between `size` and `size / 2`.
            size /= 2;
            if size < min_new_chunk_size {
                return None;
            }
        };

        debug_assert!(is_pointer_aligned_to(new_start_ptr, layout.align()));

        // Sync `Arena::cursor_ptr` back to the retiring chunk's footer so iteration over chunks
        // can read its final cursor position later. Skip this if there was no current chunk.
        if let Some(footer_ptr) = current_footer_ptr {
            // SAFETY: `footer_ptr` always points to a valid `ChunkFooter`
            let footer = unsafe { footer_ptr.as_ref() };
            footer.cursor_ptr.set(self.cursor_ptr.get());
        }

        // Set the new chunk as our new current chunk, and sync `start_ptr` and `cursor_ptr` accordingly.
        // Initial cursor sits at the footer (end of the allocatable region).
        // The footer is aligned on `CHUNK_ALIGN >= MIN_ALIGN`, so no rounding is needed.
        self.start_ptr.set(new_start_ptr);
        self.cursor_ptr.set(new_footer_ptr.cast::<u8>());
        self.current_chunk_footer_ptr.set(Some(new_footer_ptr));

        // And then we can rely on `try_alloc_layout_fast` to allocate space within this chunk
        let ptr = self.try_alloc_layout_fast(layout);
        debug_assert!(ptr.is_some());
        ptr
    }
}
