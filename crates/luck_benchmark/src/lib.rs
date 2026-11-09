//! Internal benchmark harness.
//!
//! One criterion bench binary per pipeline stage, so each stage's numbers
//! move independently. Run locally with `cargo bench -p luck_benchmark`. CI
//! runs the same binaries under `CodSpeed` with the `codspeed` feature.

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

mod corpus;
mod corpus_cache;
#[cfg(test)]
mod fixtures;
mod never_grow_in_place_allocator;

// Every bench imports criterion from here, which links this crate, and with
// it the global allocator, into benches that use nothing else of it.
pub use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

pub use corpus::{
    ProjectFile, TestFile, TestProject, bundle_project_root, test_files, test_projects,
};
