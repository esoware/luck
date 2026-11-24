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
//! Each module is read and its requires resolved by a task on rayon's pool,
//! into the arena of the thread that runs it, so that pool's threads need
//! [`luck_ast::MIN_STACK_BYTES`] of stack. The bundle is the same however
//! many threads there are.
//!
//! # Usage
//!
//! ```no_run
//! use std::path::Path;
//!
//! use luck_allocator::ThreadAllocators;
//! use luck_ast::MIN_STACK_BYTES;
//! use luck_bundler::{BundleOptions, DynamicRequire};
//! use luck_token::LuaVersion;
//!
//! rayon::ThreadPoolBuilder::new().stack_size(MIN_STACK_BYTES).build_global().unwrap();
//! let options = BundleOptions {
//!     version: LuaVersion::Lua54,
//!     search_paths: &[],
//!     project_root: Path::new("."),
//!     dynamic_require: DynamicRequire::default(),
//! };
//! let arenas = ThreadAllocators::new();
//! let bundle = luck_bundler::bundle(&arenas, Path::new("main.lua"), &options).unwrap();
//! assert!(!bundle.output.is_empty());
//! ```

mod analysis;
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

pub use bundle::{BundleResult, MinifiedBundle, bundle, bundle_minified};
pub use dynamic_require::DynamicRequire;
pub use emitter::LineMapEntry;
pub use exports::Exports;
pub use graph::{BundleOptions, DependencyGraph};
pub use module::{Dependency, ModuleId, ModuleInfo};
