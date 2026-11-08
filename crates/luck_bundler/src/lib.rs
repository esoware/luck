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
//! use luck_core::DynamicRequire;
//! use luck_token::LuaVersion;
//!
//! let bundle = luck_bundler::bundle(
//!     Path::new("main.lua"),
//!     LuaVersion::Lua54,
//!     &[],
//!     Path::new("."),
//!     DynamicRequire::default(),
//! )
//! .unwrap();
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
mod emitter;
mod graph;
mod hot_comments;
mod loader;
mod module;
mod require_extraction;

pub use bundle::{BundleResult, LineMapEntry, bundle};
pub use graph::DependencyGraph;
pub use module::{Dependency, ModuleId, ModuleInfo};
