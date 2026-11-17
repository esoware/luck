use luck_allocator::Allocator;
use luck_ast::{Block, MAX_NESTING_DEPTH, block_nesting_depth};
use luck_token::{
    Comment, CommentKind, LuaVersion, SourceError, chunk_header, leading_hot_comments,
};

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
/// Returns the parse errors when `source` doesn't parse.
pub fn minify<'ast>(
    allocator: &'ast Allocator,
    source: &'ast str,
    version: LuaVersion,
    config: &TransformConfig,
) -> Result<String, Vec<SourceError>> {
    let mut result = luck_parser::parse(allocator, source, version);
    if !result.errors.is_empty() {
        return Err(result.errors);
    }
    let shebang = result
        .comments
        .first()
        .filter(|comment| comment.kind == CommentKind::Shebang)
        .map(|comment| comment.span);
    let hot_comments: &[Comment] = if version.is_luau() { &result.comments } else { &[] };
    let mut output = chunk_header(
        source,
        shebang,
        leading_hot_comments(hot_comments, source).map(|comment| comment.span),
    );
    if !minify_tree(allocator, &mut result.block, version, config, false) {
        return Err(vec![nesting_error(&result.block)]);
    }
    let code = luck_codegen::compact(&result.block);
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
/// Returns an error for a tree nested deeper than [`MAX_NESTING_DEPTH`],
/// which `luck_parser` never builds, and leaves the tree as it was.
pub fn minify_block<'ast>(
    allocator: &'ast Allocator,
    block: &mut Block<'ast>,
    version: LuaVersion,
    config: &TransformConfig,
) -> Result<String, SourceError> {
    if !minify_tree(allocator, block, version, config, true) {
        return Err(nesting_error(block));
    }
    Ok(luck_codegen::compact(block))
}

fn nesting_error(block: &Block<'_>) -> SourceError {
    SourceError {
        span: block.span,
        message: format!(
            "maximum nesting depth exceeded (luck resource limit: {MAX_NESTING_DEPTH})"
        ),
    }
}

/// Binds the tree once, compresses it to its fixpoint, and names its locals
/// once, as each phase is on, or leaves a tree nested deeper than
/// [`MAX_NESTING_DEPTH`] as it was and says so.
///
/// Without `can_nest_too_deep`, for a tree `luck_parser` built, which never
/// nests that deep, it spares the work of keeping the tree as it was.
fn minify_tree<'ast>(
    allocator: &'ast Allocator,
    block: &mut Block<'ast>,
    version: LuaVersion,
    config: &TransformConfig,
    can_nest_too_deep: bool,
) -> bool {
    let should_compress = config.remove_dead_code
        || config.simplify_statements
        || config.fold_constants
        || config.inline_locals
        || config.merge_locals
        || config.simplify_indexes
        || config.shorten_strings
        || config.shorten_numbers
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
        bind(block, version, can_nest_too_deep, &mut bindings, &mut names)
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
