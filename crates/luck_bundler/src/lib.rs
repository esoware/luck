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
//! Modules are read and their requires resolved on one thread per arena the
//! caller passes, and the bundle is the same however many there are.
//!
//! # Usage
//!
//! ```no_run
//! use std::iter;
//! use std::path::Path;
//!
//! use luck_allocator::Allocator;
//! use luck_bundler::{BundleOptions, DynamicRequire};
//! use luck_token::LuaVersion;
//!
//! let mut arenas: Vec<Allocator> = iter::repeat_with(Allocator::new).take(8).collect();
//! let options = BundleOptions {
//!     version: LuaVersion::Lua54,
//!     search_paths: &[],
//!     project_root: Path::new("."),
//!     dynamic_require: DynamicRequire::default(),
//! };
//! let bundle = luck_bundler::bundle(&mut arenas, Path::new("main.lua"), &options).unwrap();
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

mod block_emitter;
mod bundle;
mod discovery;
mod dynamic_require;
mod emitter;
mod exports;
mod graph;
mod loader;
mod module;
mod relative_path;
mod require_extraction;

pub use bundle::{BundleResult, MinifiedBundle, bundle, bundle_minified};
pub use dynamic_require::DynamicRequire;
pub use emitter::LineMapEntry;
pub use exports::Exports;
pub use graph::{BundleOptions, DependencyGraph};
pub use module::{Dependency, ModuleId, ModuleInfo};
