use std::path::Path;

use luck_core::{Diagnostic, DynamicRequire};
use luck_token::LuaVersion;

use crate::emitter;
use crate::graph::DependencyGraph;

#[derive(Debug)]
pub struct BundleResult {
    pub output: String,
    pub warnings: Vec<Diagnostic>,
    /// Every bundled module's canonical path, in discovery order.
    pub source_files: Vec<String>,
    pub line_map: Vec<LineMapEntry>,
}

/// One contiguous run of bundle lines that came from a single module.
///
/// Line 1 of the module's source lands on `bundle_start_line`, so a
/// runtime traceback line `L` inside the range maps back to source line
/// `L - bundle_start_line + 1`. Require rewrites preserve the newline
/// count of the call they replace, so the correspondence is 1:1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMapEntry {
    /// 1-based first bundle line of the module body (inclusive).
    pub bundle_start_line: usize,
    /// 1-based last bundle line of the module body (inclusive).
    pub bundle_end_line: usize,
    /// The module's canonical path. The line map is local debug data, so it
    /// may name the host.
    pub path: String,
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
