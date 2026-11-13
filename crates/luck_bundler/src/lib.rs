//! # `luck_bundler`
//!
//! Dependency graph construction and single-file bundling for Lua and Luau
//! projects.
//!
//! Starting from an entry file, resolves every `require()` through
//! [`luck_resolver`], builds and sorts the dependency graph, and emits one file
//! where each module registers with a lazy loader that mirrors the target
//! version's own `require`.
//!
//! # Usage
//!
//! ```no_run
//! use std::path::Path;
//!
//! use luck_bundler::{BundleOptions, DynamicRequire};
//! use luck_token::LuaVersion;
//!
//! let options = BundleOptions {
//!     version: LuaVersion::Lua54,
//!     search_paths: &[],
//!     project_root: Path::new("."),
//!     dynamic_require: DynamicRequire::default(),
//! };
//! let bundle = luck_bundler::bundle(Path::new("main.lua"), &options).unwrap();
//! assert!(!bundle.output.is_empty());
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod bundle;
mod dynamic_require;
mod emitter;
mod exports;
mod graph;
mod loader;
mod module;
mod relative_path;
mod require_extraction;

pub use bundle::{BundleResult, bundle};
pub use dynamic_require::DynamicRequire;
pub use emitter::LineMapEntry;
pub use graph::{BundleOptions, DependencyGraph};
pub use module::{Dependency, ModuleId, ModuleInfo};
