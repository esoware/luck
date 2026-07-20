//! Internal benchmark harness. One criterion bench binary per pipeline
//! stage, so each stage's numbers move independently. Run locally with
//! `cargo bench -p luck_benchmark`; CI runs the same binaries under
//! CodSpeed via the `codspeed` feature.

use std::alloc::{GlobalAlloc, Layout};

use mimalloc_safe::MiMalloc;

pub use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

pub mod corpus;

#[global_allocator]
static GLOBAL: NeverGrowInPlaceAllocator = NeverGrowInPlaceAllocator;

/// Global allocator for use in benchmarks.
///
/// A thin wrapper around [`MiMalloc`] - the CLI's production allocator,
/// so bench numbers reflect the allocation paths users actually run. It
/// passes through `alloc` and `dealloc`, but does not implement
/// [`GlobalAlloc::realloc`].
///
/// A native `realloc` may either grow the allocation in place or move
/// it, depending on the state of the allocator's memory tables, which is
/// inherently non-deterministic and produces large variance in
/// benchmarks. By not providing a `realloc` method, this allocator falls
/// back to the default implementation which never grows in place: the
/// consistent worst case, so results are stable.
struct NeverGrowInPlaceAllocator;

// SAFETY: Methods simply delegate to the `MiMalloc` allocator.
unsafe impl GlobalAlloc for NeverGrowInPlaceAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { MiMalloc.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { MiMalloc.dealloc(ptr, layout) };
    }
}
