use luck_allocator::Allocator;
use luck_ast::{Chunk, MAX_NESTING_DEPTH, Name, NameIndex, TreeCounts, VisitMut};
use luck_codegen::LiteralSpelling;
use luck_diagnostics::Diagnostic;
use luck_token::{CommentKind, LuaVersion, chunk_header, leading_hot_comments};

use crate::binding::NameIndexes;
use crate::compress::compress;
use crate::rename_locals::rename;
use crate::strip_types::strip_types;
use crate::transform_config::TransformConfig;

/// Parses `source` into `allocator` and minifies it, returning the compact
/// output or the parse errors.
///
/// The source and the output are bytes, as Lua reads them. A string spelled
/// as written keeps its bytes, and the output is UTF-8 whenever the source
/// is.
///
/// # Errors
///
/// Returns the parse errors, each an E008, when `source` doesn't parse.
pub fn minify<'ast>(
    allocator: &'ast Allocator,
    source: &'ast [u8],
    version: LuaVersion,
    config: &TransformConfig,
) -> Result<Vec<u8>, Vec<Diagnostic>> {
    let mut result = luck_parser::parse(allocator, source, version);
    if !result.errors.is_empty() {
        return Err(result.errors);
    }
    let shebang = result
        .comments
        .first()
        .filter(|comment| comment.kind == CommentKind::Shebang)
        .map(|comment| comment.span);
    let mut output = chunk_header(
        source,
        shebang,
        leading_hot_comments(&result.comments, result.chunk.block.span.start)
            .map(|comment| comment.span),
    );
    let comment_bytes: usize =
        result.comments.iter().map(|comment| comment.span.text(source).len()).sum();
    let _unmet = output.try_reserve_exact(source.len().saturating_sub(comment_bytes));
    minify_tree(allocator, &mut result.chunk, result.counts, config);
    Ok(luck_codegen::compact_into(&result.chunk, LiteralSpelling::Shortest, output))
}

/// Minifies a tree in place, as [`minify`] does the one it parses, and
/// writes its compact output after the bytes `output` holds.
///
/// New nodes go in `allocator`. A tree holds no comments, so a shebang or
/// Luau hot comments the output needs on top are the caller's to write into
/// `output` first.
///
/// # Errors
///
/// Returns an E008 for a tree nested deeper than [`MAX_NESTING_DEPTH`],
/// which `luck_parser` never builds, and leaves the tree as it was but for
/// its names' indexes.
pub fn minify_chunk<'ast>(
    allocator: &'ast Allocator,
    chunk: &mut Chunk<'ast>,
    config: &TransformConfig,
    output: Vec<u8>,
) -> Result<Vec<u8>, Diagnostic> {
    let mut numbering = NameNumbering { names: 0, depth: 0, deepest: 0 };
    numbering.visit_block(&mut chunk.block);
    if numbering.deepest > MAX_NESTING_DEPTH {
        return Err(Diagnostic::e008(
            chunk.block.span,
            format!("maximum nesting depth exceeded (luck resource limit: {MAX_NESTING_DEPTH})"),
        ));
    }
    let counts = TreeCounts { names: numbering.names, ..TreeCounts::default() };
    minify_tree(allocator, chunk, counts, config);
    Ok(luck_codegen::compact_into(chunk, LiteralSpelling::Shortest, output))
}

/// Strips a Luau tree's types, compresses it, and names its locals, as each
/// phase is on. Every name of the tree holds an index of its own, below
/// `counts.names`.
fn minify_tree<'ast>(
    allocator: &'ast Allocator,
    chunk: &mut Chunk<'ast>,
    counts: TreeCounts,
    config: &TransformConfig,
) {
    if config.strip_types && chunk.version.has_type_syntax() {
        strip_types(allocator, &mut chunk.block);
    }
    let mut names = NameIndexes::new(counts.names);
    if config.compresses() {
        compress(allocator, chunk, counts, *config, &mut names);
    }
    if config.rename_locals {
        rename(allocator, chunk, counts, config.rename_globals, &mut names);
    }
}

/// Gives every name of a tree an index of its own, as the parser does, for
/// a tree that joins nodes from several parses or builds some in code, and
/// measures how deep it nests.
struct NameNumbering {
    names: u32,
    /// Levels of nesting open where the walk is, and the most there have
    /// been.
    depth: usize,
    deepest: usize,
}

impl<'ast> VisitMut<'ast> for NameNumbering {
    /// The walk goes no deeper than one level past the cap, so it's safe on a
    /// tree of any depth.
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        self.deepest = self.deepest.max(self.depth);
        self.depth <= MAX_NESTING_DEPTH
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_name(&mut self, name: &mut Name<'ast>) {
        name.index = Some(NameIndex::new(self.names));
        self.names = self.names.saturating_add(1);
    }
}
