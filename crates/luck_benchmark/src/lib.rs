//! Internal benchmark harness. One criterion bench binary per pipeline
//! stage, so each stage's numbers move independently. Run locally with
//! `cargo bench -p luck_benchmark`; CI runs the same binaries under
//! CodSpeed via the `codspeed` feature.

use std::alloc::{GlobalAlloc, Layout, System};

pub use criterion::*;

pub mod corpus;

#[global_allocator]
static GLOBAL: NeverGrowInPlaceAllocator = NeverGrowInPlaceAllocator;

/// Global allocator for use in benchmarks.
///
/// A thin wrapper around Rust's default [`System`] allocator. It passes
/// through `alloc` and `dealloc` to [`System`], but does not implement
/// [`GlobalAlloc::realloc`].
///
/// `realloc` on the [`System`] allocator may either grow the allocation
/// in place or move it, depending on the state of the operating system's
/// memory tables, which is inherently non-deterministic and produces
/// large variance in benchmarks. By not providing a `realloc` method,
/// this allocator falls back to the default implementation which never
/// grows in place: the consistent worst case, so results are stable.
struct NeverGrowInPlaceAllocator;

// SAFETY: Methods simply delegate to the `System` allocator.
unsafe impl GlobalAlloc for NeverGrowInPlaceAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }
}
