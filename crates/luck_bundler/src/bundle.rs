use core::mem;
use std::path::Path;

use luck_core::Diagnostic;
use luck_minifier::TransformConfig;

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

#[derive(Debug)]
pub struct MinifiedBundle {
    /// Opens with the entry's shebang and its Luau hot comments.
    pub output: String,
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
/// module's wrapper would nest past the dialect's
/// [`max_nesting_depth`](luck_token::LuaVersion::max_nesting_depth).
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

/// The bundle [`bundle`] writes, minified from the tree that holds each
/// module's own parse, so its text is never parsed again.
///
/// # Errors
///
/// Returns what [`bundle`] does.
#[expect(clippy::missing_panics_doc, reason = "the emitter refuses a tree past the cap")]
pub fn bundle_minified(
    entry_path: &Path,
    options: &BundleOptions<'_>,
    transforms: &TransformConfig,
) -> Result<MinifiedBundle, Vec<Diagnostic>> {
    let mut graph = DependencyGraph::build(entry_path, options)?;
    let warnings = mem::take(&mut graph.warnings);
    let source_files = graph.modules.iter().map(|module| module.path.clone()).collect();
    let source_byte_count = graph.modules.iter().map(|module| module.source.len()).sum();
    let (block, mut output) = block_emitter::emit_block(graph, options.version)?;
    #[expect(
        clippy::expect_used,
        clippy::unwrap_in_result,
        reason = "the emitter refuses a tree past the cap"
    )]
    let code = luck_minifier::minify_block(block, options.version, transforms)
        .expect("a bundle nests within the cap");
    output.push_str(&code);
    Ok(MinifiedBundle { output, warnings, source_files, source_byte_count })
}
