use luck_allocator::Allocator;
use luck_ast::{Chunk, MAX_NESTING_DEPTH, block_nesting_depth};
use luck_diagnostics::Diagnostic;
use luck_token::{CommentKind, LuaVersion, chunk_header, leading_hot_comments};

use crate::bind::bind;
use crate::binding::Bindings;
use crate::compress::compress;
use crate::name_table::NameTable;
use crate::rename_locals::rename;
use crate::transform_config::TransformConfig;

/// Parses `source` into `allocator` and minifies it, returning the compact
/// output or the parse errors.
///
/// # Errors
///
/// Returns the parse errors, each an E008, when `source` doesn't parse.
pub fn minify<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
    config: &TransformConfig,
) -> Result<String, Vec<Diagnostic>> {
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
    if !minify_tree(allocator, &mut result.chunk, config, false) {
        return Err(vec![nesting_error(&result.chunk)]);
    }
    let code = luck_codegen::compact(&result.chunk);
    if output.is_empty() {
        return Ok(code);
    }
    output.push_str(&code);
    Ok(output)
}

/// Minifies a tree in place, as [`minify`] does the one it parses, and
/// returns its compact output.
///
/// New nodes go in `allocator`. A tree holds no comments, so a shebang or
/// Luau hot comments the output needs on top are the caller's to write.
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
) -> Result<String, Diagnostic> {
    if !minify_tree(allocator, chunk, config, true) {
        return Err(nesting_error(chunk));
    }
    Ok(luck_codegen::compact(chunk))
}

fn nesting_error(chunk: &Chunk<'_>) -> Diagnostic {
    Diagnostic::e008(
        chunk.block.span,
        format!("maximum nesting depth exceeded (luck resource limit: {MAX_NESTING_DEPTH})"),
    )
}

/// Binds the tree once, compresses it to its fixpoint, and names its locals
/// once, as each phase is on, or leaves a tree nested deeper than
/// [`MAX_NESTING_DEPTH`] as [`minify_chunk`] does and says so.
///
/// Without `can_nest_too_deep`, for a tree `luck_parser` built, which never
/// nests that deep, it spares measuring a tree it doesn't bind.
fn minify_tree<'ast>(
    allocator: &'ast Allocator,
    chunk: &mut Chunk<'ast>,
    config: &TransformConfig,
    can_nest_too_deep: bool,
) -> bool {
    let version = chunk.version;
    let block = &mut chunk.block;
    let should_compress = config.remove_dead_code
        || config.simplify_statements
        || config.fold_constants
        || config.inline_locals
        || config.merge_locals
        || config.simplify_indexes
        || config.simplify_parens
        || config.lift_locals;
    let needs_bindings = config.remove_dead_code
        || config.inline_locals
        || config.merge_locals
        || config.lift_locals
        || config.rename_locals;
    let mut bindings = Bindings::default();
    let mut names = NameTable::default();
    let fits = if needs_bindings {
        bind(block, version, &mut bindings, &mut names)
    } else {
        !can_nest_too_deep || block_nesting_depth(block) <= MAX_NESTING_DEPTH
    };
    if !fits {
        return false;
    }
    if should_compress {
        compress(allocator, version, *config, &mut bindings, block);
    }
    if config.rename_locals {
        rename(allocator, version, block, &mut bindings, &names, config.rename_globals);
    }
    true
}
