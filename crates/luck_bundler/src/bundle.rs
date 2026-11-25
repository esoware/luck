use std::path::{Path, PathBuf};

use luck_allocator::ThreadAllocators;
use luck_diagnostics::FileDiagnostics;
use luck_minifier::TransformConfig;

use crate::graph::{BundleOptions, DependencyGraph};
use crate::text_emitter::{self, LineMapEntry};
use crate::tree_emitter;

#[derive(Debug)]
pub struct Bundle {
    /// Parses as Luau, so every luck tool can read it back. Bytes, as the
    /// modules' sources are, which it copies as written.
    pub output: Vec<u8>,
    /// By module, in discovery order.
    pub warnings: Vec<FileDiagnostics>,
    /// Every bundled module's canonical path, in discovery order.
    pub source_files: Vec<PathBuf>,
    pub line_map: Vec<LineMapEntry>,
}

#[derive(Debug)]
pub struct MinifiedBundle {
    /// Opens with the entry's shebang and its hot comments.
    pub output: Vec<u8>,
    /// By module, in discovery order.
    pub warnings: Vec<FileDiagnostics>,
    /// Every bundled module's canonical path, in discovery order.
    pub source_files: Vec<PathBuf>,
    /// The bundled modules' sources' total size.
    pub source_byte_count: usize,
}

/// Bundles the modules `entry_path` reaches, whose sources and trees go in
/// `arenas` as [`DependencyGraph::build`] puts them there, and the nodes it
/// builds in the calling thread's.
///
/// # Errors
///
/// Returns every diagnostic that stopped the graph from building, as
/// [`DependencyGraph::build`] does, or an E013 for each construct that a
/// module's wrapper would nest past [`luck_ast::MAX_NESTING_DEPTH`].
pub fn bundle(
    arenas: &ThreadAllocators,
    entry_path: &Path,
    options: &BundleOptions<'_>,
) -> Result<Bundle, Vec<FileDiagnostics>> {
    let mut graph = DependencyGraph::build(arenas, entry_path, options)?;
    let (output, line_map) = text_emitter::emit(arenas.get(), &mut graph)?;
    Ok(Bundle {
        output,
        warnings: graph.warnings,
        source_files: graph.modules.into_iter().map(|module| module.path).collect(),
        line_map,
    })
}

/// The bundle [`bundle`] writes, minified from the tree that holds each
/// module's own parse, so its text is never parsed again. The nodes it builds
/// go in the calling thread's arena.
///
/// # Errors
///
/// Returns what [`bundle`] does.
#[expect(clippy::missing_panics_doc, reason = "the emitter refuses a tree past the cap")]
pub fn bundle_minified(
    arenas: &ThreadAllocators,
    entry_path: &Path,
    options: &BundleOptions<'_>,
    transforms: &TransformConfig,
) -> Result<MinifiedBundle, Vec<FileDiagnostics>> {
    let mut graph = DependencyGraph::build(arenas, entry_path, options)?;
    let allocator = arenas.get();
    let (mut chunk, header) = tree_emitter::emit(allocator, &mut graph)?;
    let output = luck_minifier::minify_chunk(allocator, &mut chunk, transforms, header)
        .expect("a bundle nests within the cap");
    Ok(MinifiedBundle {
        output,
        warnings: graph.warnings,
        source_byte_count: graph.modules.iter().map(|module| module.source.len()).sum(),
        source_files: graph.modules.into_iter().map(|module| module.path).collect(),
    })
}
