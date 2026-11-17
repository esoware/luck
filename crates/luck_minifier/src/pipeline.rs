use luck_allocator::Allocator;
use luck_ast::{Block, MAX_NESTING_DEPTH};
use luck_token::{
    Comment, CommentKind, LuaVersion, SourceError, chunk_header, leading_hot_comments,
};

use crate::compress::Compressor;
use crate::lift_locals::Lifter;
use crate::rename_locals::Renamer;
use crate::transform_config::TransformConfig;

/// Two or three rounds settle every measured input, so the cap only bounds
/// a phase that never stops changing the tree, which the fixpoint tests
/// catch.
const MAX_ROUNDS: usize = 8;

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
    let compressor = Compressor::new(allocator, version, *config);
    minify_tree(compressor, allocator, &mut result.block, version, config);
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
    let mut compressor = Compressor::new(allocator, version, *config);
    if !compressor.fits_nesting_cap(block) {
        return Err(SourceError {
            span: block.span,
            message: format!(
                "maximum nesting depth exceeded (luck resource limit: {MAX_NESTING_DEPTH})"
            ),
        });
    }
    minify_tree(compressor, allocator, block, version, config);
    Ok(luck_codegen::compact(block))
}

fn minify_tree<'ast>(
    mut compressor: Compressor<'ast>,
    allocator: &'ast Allocator,
    block: &mut Block<'ast>,
    version: LuaVersion,
    config: &TransformConfig,
) {
    let mut renamer = Renamer::new(allocator, version);
    let mut lifter = Lifter::new(allocator, version);
    let should_rename = config.rename_locals;
    let should_lift = config.lift_locals;
    let mut has_lifted = false;
    for round in 0..MAX_ROUNDS {
        let has_compressed = compressor.compress(block);
        // Renaming changes nothing it didn't change last round unless
        // something has changed since.
        if round > 0 && !has_compressed && !has_lifted {
            break;
        }
        if !should_rename && !should_lift {
            break;
        }
        // Methods take an explicit self on the first rename, so the renamer
        // can shorten the parameter.
        let has_renamed = should_rename
            && renamer.rename(block, compressor.names(), config.rename_globals, round == 0);
        // Lift decides by name, and rename's slot reuse gives distinct
        // bindings the same name, so each round can unblock more lifts.
        has_lifted = should_lift && compressor.lift(block, &mut lifter);
        if !has_renamed && !has_lifted {
            break;
        }
    }
}
