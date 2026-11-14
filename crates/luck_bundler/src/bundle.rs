use core::mem;
use std::path::Path;

use luck_ast::Block;
use luck_core::Diagnostic;

use crate::block_emitter;
use crate::emitter::{self, LineMapEntry};
use crate::graph::{BundleOptions, DependencyGraph};

#[derive(Debug)]
pub struct BundleResult {
    /// Parses for the target, so every luck tool can read it back.
    pub output: String,
    pub warnings: Vec<Diagnostic>,
    /// Every bundled module's canonical path, in discovery order.
    pub source_files: Vec<String>,
    pub line_map: Vec<LineMapEntry>,
}

/// The bundle [`bundle`] writes, as a tree, for a caller such as the
/// minifier that works on one: each module keeps the tree its own parse
/// gave it, rewritten as the text splices it.
#[derive(Debug)]
pub struct BlockBundle {
    /// The bundle's text parses to this, apart from spans, comments, and `;`.
    pub block: Block,
    /// The lines a printed bundle opens with, each ending in a line break: the
    /// entry's shebang and its Luau hot comments, which take effect only
    /// there.
    pub header: String,
    pub warnings: Vec<Diagnostic>,
    /// Every bundled module's canonical path, in discovery order.
    pub source_files: Vec<String>,
    /// The bundled modules' sources' total size.
    pub source_byte_count: usize,
}

/// # Errors
///
/// Returns every diagnostic that stopped the graph from building, as
/// [`DependencyGraph::build`] does, or an E013 for each construct that a
/// module's wrapper would nest past [`luck_ast::MAX_NESTING_DEPTH`].
pub fn bundle(
    entry_path: &Path,
    options: &BundleOptions<'_>,
) -> Result<BundleResult, Vec<Diagnostic>> {
    let graph = DependencyGraph::build(entry_path, options)?;
    let (output, line_map) = emitter::emit(&graph, options.version)?;
    Ok(BundleResult {
        output,
        warnings: graph.warnings,
        source_files: graph.modules.into_iter().map(|module| module.path).collect(),
        line_map,
    })
}

/// # Errors
///
/// Returns what [`bundle`] does.
pub fn bundle_block(
    entry_path: &Path,
    options: &BundleOptions<'_>,
) -> Result<BlockBundle, Vec<Diagnostic>> {
    let mut graph = DependencyGraph::build(entry_path, options)?;
    let warnings = mem::take(&mut graph.warnings);
    let source_files = graph.modules.iter().map(|module| module.path.clone()).collect();
    let source_byte_count = graph.modules.iter().map(|module| module.source.len()).sum();
    let (block, header) = block_emitter::emit_block(graph, options.version)?;
    Ok(BlockBundle { block, header, warnings, source_files, source_byte_count })
}
