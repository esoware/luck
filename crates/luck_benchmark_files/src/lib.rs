//! Internal test inputs.
//!
//! The pinned files and projects that the benches, the minified-size
//! snapshot, and the allocation snapshots all measure, downloaded once into
//! the gitignored `cache/` directory next to this crate's `Cargo.toml`.
//! `luck_conformance` downloads and unpacks its suite with the same
//! [`fetch_bytes`] and [`extract_once`].

mod benchmark_files;
mod download;

pub use benchmark_files::{
    BundleProject, ProjectFile, TestFile, TestProject, bundle_projects, test_files, test_projects,
};
pub use download::{extract_once, fetch_bytes};
