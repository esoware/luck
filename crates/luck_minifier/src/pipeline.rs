use core::mem;

use luck_ast::Block;
use luck_core::TransformConfig;
use luck_token::{LuaVersion, SourceError, leading_hot_comments};

use crate::transforms;

/// Round cap for the outer pipeline loop and the tail's inner loop.
///
/// Measured corpora converge in two outer rounds, so the cap only bounds
/// pathological oscillation.
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
    // Luau hot comments (--!strict, --!native, --!optimize N, ...) only take
    // effect before any code, so the leading ones go back on top of the
    // output.
    let hot_comment_prefix = if version.is_luau() {
        let mut prefix = String::new();
        for comment in leading_hot_comments(&result.comments, source) {
            prefix.push_str(comment.span.text(source).trim_end());
            prefix.push('\n');
        }
        prefix
    } else {
        String::new()
    };
    let mut block = result.block;

    // The tail runs to its own fixpoint, so measured corpora converge here in
    // two rounds: one does the work and one proves it stable.
    //
    // Each round that changes the output reparses it for the next round.
    // Transform-built nodes aren't always shaped like their parsed
    // equivalents (paren wrappers, span-less tokens), so a pass matching the
    // parsed shape could miss work that a round trip would expose. Converging
    // on emitted text is what keeps minify(minify(x)) == minify(x).
    let mut previous_output = luck_codegen::compact(&block);
    for _ in 0..MAX_PIPELINE_ROUNDS {
        block = apply_core_transforms(block, config, version);
        block = apply_tail_transforms(block, config, version, previous_output.len());
        let output = luck_codegen::compact_with_capacity(&block, previous_output.len());
        if output == previous_output {
            break;
        }
        // The parser hands its owned source back, so the round's output isn't
        // copied to reparse it.
        let reparsed = luck_parser::parse(output, version);
        previous_output = reparsed.source;
        debug_assert!(
            reparsed.errors.is_empty(),
            "minified round output failed to reparse: {:?}\noutput:\n{previous_output}",
            reparsed.errors
        );
        if !reparsed.errors.is_empty() {
            break;
        }
        block = reparsed.block;
    }

    if hot_comment_prefix.is_empty() {
        Ok(previous_output)
    } else {
        Ok(format!("{hot_comment_prefix}{previous_output}"))
    }
}

/// One round of the cheap peephole passes, in dependency order.
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

/// The expensive, ordering-sensitive tail of the pipeline.
///
/// Lift decides by name, and rename's slot reuse gives distinct bindings the
/// same name, so each rename, lift, and merge round can unblock more lifts.
/// The loop runs that cascade to its own fixpoint here, so the outer loop
/// never reruns the core passes or reparses for tail-only work.
fn apply_tail_transforms(
    mut block: Block,
    config: &TransformConfig,
    version: LuaVersion,
    output_capacity: usize,
) -> Block {
    if !config.rename_locals && !config.lift_locals {
        return block;
    }

    // Explicit self goes before rename so the renamer can shorten the parameter.
    if config.rename_locals {
        block = transforms::explicit_self::rewrite(block);
    }

    // The output doesn't shrink monotonically from one iteration to the
    // next, so keep the smallest emit seen instead of wherever the loop
    // stops. The last iteration is usually the best, so the reparse below
    // rarely runs. Outputs move between the two slots instead of being
    // copied, since each one is the size of the whole chunk.
    let mut last_output = luck_codegen::compact_with_capacity(&block, output_capacity);
    // The shortest output of an iteration before the last, the first on ties.
    let mut best_earlier_output: Option<String> = None;
    for round in 0..MAX_PIPELINE_ROUNDS {
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

        let output = luck_codegen::compact_with_capacity(&block, last_output.len());
        if output == last_output {
            break;
        }
        let earlier_output = mem::replace(&mut last_output, output);
        // The first round's predecessor is the emit from before the tail ran,
        // which isn't a candidate.
        let is_shorter =
            best_earlier_output.as_ref().is_none_or(|best| earlier_output.len() < best.len());
        if round > 0 && is_shorter {
            best_earlier_output = Some(earlier_output);
        }
    }
    let Some(best_output) = best_earlier_output.filter(|best| best.len() < last_output.len())
    else {
        return block;
    };
    let reparsed = luck_parser::parse(best_output, version);
    if reparsed.errors.is_empty() {
        reparsed.block
    } else {
        debug_assert!(false, "tail best output failed to reparse: {}", reparsed.source);
        block
    }
}
