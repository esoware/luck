use core::alloc::{GlobalAlloc, Layout};

use mimalloc_safe::MiMalloc;

#[global_allocator]
static GLOBAL: NeverGrowInPlaceAllocator = NeverGrowInPlaceAllocator;

/// Global allocator for the benchmarks.
///
/// A thin wrapper around [`MiMalloc`], the CLI's production allocator, so
/// bench numbers reflect the allocation paths users run. It passes through
/// `alloc` and `dealloc` but leaves out [`GlobalAlloc::realloc`].
///
/// Whether a native `realloc` grows in place or moves depends on the
/// allocator's internal state, which adds a lot of variance between runs.
/// The default `realloc` always allocates, copies, and frees, a consistent
/// worst case that keeps results stable.
struct NeverGrowInPlaceAllocator;

#[expect(unsafe_code, reason = "a global allocator can only be written with unsafe")]
// SAFETY: Both methods delegate to the `MiMalloc` allocator.
unsafe impl GlobalAlloc for NeverGrowInPlaceAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller's `GlobalAlloc::alloc` contract is forwarded
        // unchanged.
        unsafe { MiMalloc.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `alloc` above is the only source of `ptr`, so it belongs to
        // `MiMalloc` and `layout` is the layout it was allocated with.
        unsafe {
            MiMalloc.dealloc(ptr, layout);
        }
    }
}
