use std::path::Path;

use luck_core::{Diagnostic, DynamicRequire};
use luck_token::LuaVersion;

use crate::emitter::{self, LineMapEntry};
use crate::graph::DependencyGraph;

#[derive(Debug)]
pub struct BundleResult {
    pub output: String,
    pub warnings: Vec<Diagnostic>,
    /// Every bundled module's canonical path, in discovery order.
    pub source_files: Vec<String>,
    pub line_map: Vec<LineMapEntry>,
}

/// Bundles `entry_path` and everything it requires into one Lua file.
///
/// Lua search paths resolve against `project_root`, and every path in the
/// output is relative to it. An empty `search_paths` means the default Lua
/// templates.
///
/// # Errors
///
/// Returns every diagnostic that stopped the graph from building, as
/// [`DependencyGraph::build`] does.
pub fn bundle(
    entry_path: &Path,
    version: LuaVersion,
    search_paths: &[String],
    project_root: &Path,
    dynamic_require: DynamicRequire,
) -> Result<BundleResult, Vec<Diagnostic>> {
    let graph =
        DependencyGraph::build(entry_path, version, search_paths, project_root, dynamic_require)?;
    let (output, line_map) = emitter::emit(&graph, version);
    Ok(BundleResult {
        output,
        warnings: graph.warnings,
        source_files: graph.modules.into_iter().map(|module| module.path).collect(),
        line_map,
    })
}
