use std::path::Path;

use luck_core::Diagnostic;

use crate::emitter::{self, LineMapEntry};
use crate::graph::{BundleOptions, DependencyGraph};

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
/// # Errors
///
/// Returns every diagnostic that stopped the graph from building, as
/// [`DependencyGraph::build`] does.
pub fn bundle(
    entry_path: &Path,
    options: &BundleOptions<'_>,
) -> Result<BundleResult, Vec<Diagnostic>> {
    let graph = DependencyGraph::build(entry_path, options)?;
    let (output, line_map) = emitter::emit(&graph, options.version);
    Ok(BundleResult {
        output,
        warnings: graph.warnings,
        source_files: graph.modules.into_iter().map(|module| module.path).collect(),
        line_map,
    })
}
