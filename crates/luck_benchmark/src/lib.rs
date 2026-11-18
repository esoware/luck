//! Internal benchmark harness.
//!
//! One criterion bench binary per pipeline stage, so each stage's numbers
//! move independently. Run locally with `cargo bench -p luck_benchmark`. CI
//! runs the same binaries under `CodSpeed` with the `codspeed` feature.

mod never_grow_in_place_allocator;

// Every bench imports criterion from here, so even one that uses nothing
// else of this crate links its global allocator.
pub use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
