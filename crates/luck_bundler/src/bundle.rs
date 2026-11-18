use core::mem;
use std::path::{Path, PathBuf};

use luck_allocator::Allocator;
use luck_core::Diagnostic;
use luck_minifier::TransformConfig;

use crate::block_emitter;
use crate::emitter::{self, LineMapEntry};
use crate::graph::{BundleOptions, build_graph};

#[derive(Debug)]
pub struct BundleResult {
    /// Parses for the target, so every luck tool can read it back.
    pub output: String,
    pub warnings: Vec<Diagnostic>,
    /// Every bundled module's canonical path, in discovery order.
    pub source_files: Vec<PathBuf>,
    pub line_map: Vec<LineMapEntry>,
}

#[derive(Debug)]
pub struct MinifiedBundle {
    /// Opens with the entry's shebang and its Luau hot comments.
    pub output: String,
    pub warnings: Vec<Diagnostic>,
    /// Every bundled module's canonical path, in discovery order.
    pub source_files: Vec<PathBuf>,
    /// The bundled modules' sources' total size.
    pub source_byte_count: usize,
}

/// Bundles the modules `entry_path` reaches, whose sources and trees go in
/// `arenas` as [`DependencyGraph::build`](crate::DependencyGraph::build) puts them there.
///
/// # Errors
///
/// Returns every diagnostic that stopped the graph from building, as
/// [`DependencyGraph::build`](crate::DependencyGraph::build) does, or an E013 for each construct that a
/// module's wrapper would nest past [`luck_ast::MAX_NESTING_DEPTH`].
///
/// # Panics
///
/// Panics if `arenas` is empty.
pub fn bundle(
    arenas: &mut [Allocator],
    entry_path: &Path,
    options: &BundleOptions<'_>,
) -> Result<BundleResult, Vec<Diagnostic>> {
    let (allocator, built) = build_graph(arenas, entry_path, options);
    let mut graph = built?;
    let (output, line_map) = emitter::emit(allocator, &mut graph, options.version)?;
    Ok(BundleResult {
        output,
        warnings: graph.warnings,
        source_files: graph.modules.into_iter().map(|module| module.path).collect(),
        line_map,
    })
}

/// The bundle [`bundle`] writes, minified from the tree that holds each
/// module's own parse, so its text is never parsed again. The nodes it builds
/// go in the first of `arenas`.
///
/// # Errors
///
/// Returns what [`bundle`] does.
///
/// # Panics
///
/// Panics if `arenas` is empty.
pub fn bundle_minified(
    arenas: &mut [Allocator],
    entry_path: &Path,
    options: &BundleOptions<'_>,
    transforms: &TransformConfig,
) -> Result<MinifiedBundle, Vec<Diagnostic>> {
    let (allocator, built) = build_graph(arenas, entry_path, options);
    let mut graph = built?;
    let warnings = mem::take(&mut graph.warnings);
    let source_files = graph.modules.iter().map(|module| module.path.clone()).collect();
    let source_byte_count = graph.modules.iter().map(|module| module.source.len()).sum();
    let (mut block, mut output) = block_emitter::emit_block(allocator, graph, options.version)?;
    #[expect(
        clippy::expect_used,
        clippy::unwrap_in_result,
        reason = "the emitter refuses a tree past the cap"
    )]
    let code = luck_minifier::minify_block(allocator, &mut block, options.version, transforms)
        .expect("a bundle nests within the cap");
    if output.is_empty() {
        output = code;
    } else {
        output.push_str(&code);
    }
    Ok(MinifiedBundle { output, warnings, source_files, source_byte_count })
}
