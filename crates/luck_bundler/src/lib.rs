//! # luck_bundler
//!
//! Dependency graph construction and single-file bundling for Lua/Luau projects.
//!
//! Starting from an entry file, resolves all `require()` calls via [`luck_resolver`],
//! builds a BFS dependency graph, topologically sorts modules, and emits each as an
//! IIFE (immediately-invoked function expression) with require calls rewritten.
//!
//! # Usage
//!
//! ```no_run
//! use luck_core::LuaTarget;
//! use std::path::Path;
//!
//! let bundle = luck_bundler::bundle(Path::new("main.lua"), LuaTarget::Lua54, &[], Path::new(".")).unwrap();
//! assert!(!bundle.output.is_empty());
//! ```

#![allow(clippy::result_large_err)]

pub mod emitter;
pub mod graph;
pub mod module;
mod require_extraction;

use luck_core::diagnostics::Diagnostic;
use luck_core::types::LuaTarget;
use std::path::Path;

/// Output of the bundling process: merged source, warnings, file list,
/// and the module->bundle line map for the emitted output.
pub struct BundleResult {
    pub output: String,
    pub warnings: Vec<Diagnostic>,
    pub source_files: Vec<String>,
    pub line_map: Vec<emitter::LineMapEntry>,
}

/// Resolves all dependencies from `entry_path` and emits a single bundled Lua file.
pub fn bundle(
    entry_path: &Path,
    target: LuaTarget,
    search_paths: &[String],
    rc_dir: &Path,
) -> Result<BundleResult, Vec<Diagnostic>> {
    let dep_graph = graph::build_graph(entry_path, target, search_paths, rc_dir)?;

    let warnings = dep_graph.warnings.clone();
    let source_files: Vec<String> = dep_graph.modules.iter().map(|m| m.path.clone()).collect();
    let (output, line_map) = emitter::emit_with_line_map(&dep_graph, target.lua_version());

    Ok(BundleResult {
        output,
        warnings,
        source_files,
        line_map,
    })
}
