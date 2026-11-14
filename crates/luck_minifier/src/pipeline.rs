use luck_ast::Block;
use luck_token::{CommentKind, LuaVersion, SourceError, leading_hot_comments};

use crate::transform_config::TransformConfig;
use crate::transforms;

/// Measured corpora settle in two or three rounds, so the cap only bounds a
/// pass that never stops changing the tree, which the fixpoint tests catch.
const MAX_PIPELINE_ROUNDS: usize = 8;

/// Minifies Lua source, returning the compact output or the parse errors.
///
/// The transform chain repeats until its output stops changing, since each
/// pass can expose work for the others: inlining opens folds, folding opens
/// dead branches, and lift and merge reshape declarations for the next round.
///
/// # Errors
///
/// Returns the parse errors when `source` doesn't parse.
pub fn minify(
    source: &str,
    version: LuaVersion,
    config: &TransformConfig,
) -> Result<String, Vec<SourceError>> {
    let result = luck_parser::parse(source, version);
    if !result.errors.is_empty() {
        return Err(result.errors);
    }
    // A shebang and Luau hot comments (--!strict, --!native, --!optimize N,
    // ...) only take effect before any code, so they go back on top of the
    // output.
    let mut output = String::new();
    if let Some(shebang) =
        result.comments.first().filter(|comment| comment.kind == CommentKind::Shebang)
    {
        // A trailing CR would pair with this line break into a CRLF.
        output.push_str(shebang.span.text(source).trim_end_matches('\r'));
        output.push('\n');
    }
    if version.is_luau() {
        for comment in leading_hot_comments(&result.comments, source) {
            output.push_str(comment.span.text(source).trim_end());
            output.push('\n');
        }
    }
    let code = minify_block(result.block, version, config).map_err(|error| vec![error])?;
    if output.is_empty() {
        return Ok(code);
    }
    output.push_str(&code);
    Ok(output)
}

/// Minifies a tree as [`minify`] does the one it parses, and returns the
/// compact output. A tree holds no comments, so a shebang or Luau hot
/// comments the output needs on top are the caller's to write.
///
/// Every round works on the tree the last one left. The passes build nodes
/// in the shape the parser gives their printed text, so the tree the rounds
/// settle on is the one its own output parses to, which is what makes
/// minifying the output again a fixpoint.
///
/// # Errors
///
/// Returns an error for a tree nested deeper than
/// [`MAX_NESTING_DEPTH`](luck_ast::MAX_NESTING_DEPTH), which `luck_parser`
/// never builds.
pub fn minify_block(
    mut block: Block,
    version: LuaVersion,
    config: &TransformConfig,
) -> Result<String, SourceError> {
    let mut output = luck_codegen::compact(&block)?;
    for _ in 0..MAX_PIPELINE_ROUNDS {
        block = apply_core_transforms(block, config, version);
        let (tail_block, round_output) =
            apply_tail_transforms(block, config, version, output.len())?;
        block = tail_block;
        if round_output == output {
            break;
        }
        output = round_output;
    }
    Ok(output)
}

fn apply_core_transforms(mut block: Block, config: &TransformConfig, version: LuaVersion) -> Block {
    if config.remove_dead_code {
        block = transforms::remove_dead_code::remove(block, version);
    }
    if config.simplify_statements {
        block = transforms::simplify_statements::simplify(block);
    }
    if config.fold_constants {
        block = transforms::fold_constants::fold(block, version);
    }
    if config.inline_locals {
        block = transforms::inline_locals::inline(block, version);
    }
    // Inlining exposes new foldable constants.
    if config.fold_constants {
        block = transforms::fold_constants::fold(block, version);
    }
    // Folding those (`local DEBUG = false` inlined into `if DEBUG then`)
    // exposes new dead branches within the same round.
    if config.remove_dead_code {
        block = transforms::remove_dead_code::remove(block, version);
    }
    if config.merge_locals {
        block = transforms::merge_locals::merge(block, version);
    }
    if config.simplify_indexes {
        block = transforms::simplify_indexes::simplify(block, version);
    }
    if config.shorten_strings {
        block = transforms::shorten_strings::shorten(block, version);
    }
    if config.shorten_numbers {
        block = transforms::shorten_numbers::shorten(block, version);
    }
    if config.simplify_parens {
        block = transforms::simplify_parens::simplify(block);
    }
    block
}

/// Lift decides by name, and rename's slot reuse gives distinct bindings the
/// same name, so each rename, lift, and merge round can unblock more lifts.
///
/// The loop runs that cascade to its own fixpoint here, so the outer loop
/// never reruns the core passes for tail-only work. Returns the tree and its
/// compact output.
///
/// # Errors
///
/// Returns the error [`luck_codegen::compact`] does.
fn apply_tail_transforms(
    mut block: Block,
    config: &TransformConfig,
    version: LuaVersion,
    output_capacity: usize,
) -> Result<(Block, String), SourceError> {
    // Explicit self goes before rename so the renamer can shorten the parameter.
    if config.rename_locals {
        block = transforms::explicit_self::rewrite(block);
    }
    let mut output = luck_codegen::compact_with_capacity(&block, output_capacity)?;
    if !config.rename_locals && !config.lift_locals {
        return Ok((block, output));
    }
    for _ in 0..MAX_PIPELINE_ROUNDS {
        if config.rename_locals {
            block = transforms::rename_locals::rename(block, version, config.rename_globals);
        }
        if config.lift_locals {
            block = transforms::lift_locals::lift(block, version);
        }
        // Fuse `local X\nX=Y` back into `local X=Y` after lifting.
        if config.rename_locals && config.merge_locals {
            block = transforms::merge_locals::merge(block, version);
        }
        let next_output = luck_codegen::compact_with_capacity(&block, output.len())?;
        if next_output == output {
            break;
        }
        output = next_output;
    }
    Ok((block, output))
}
