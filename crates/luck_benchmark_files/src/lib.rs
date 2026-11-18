//! Internal test inputs.
//!
//! The pinned files and projects that the benches, the minified-size
//! snapshot, and the allocation snapshots all measure, downloaded once into
//! the gitignored `cache/` directory next to this crate's `Cargo.toml`.

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

mod benchmark_files;
mod download;
#[cfg(test)]
mod fixtures;

pub use benchmark_files::{
    ProjectFile, TestFile, TestProject, bundle_project_root, test_files, test_projects,
};
