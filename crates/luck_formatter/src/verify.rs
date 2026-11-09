//! Verified formatting: the checks `luck fmt --verify` runs on a format.
//!
//! The output has to re-parse to an equivalent AST, keep every comment's text
//! and place among the tokens, and come back unchanged from a second format.

use alloc::borrow::Cow;
use core::mem::discriminant;

use luck_token::{Comment, LuaVersion, Token, TokenKind};

use crate::FormatOptions;
use crate::ast_equivalence::{AstDiff, blocks_equivalent};
use crate::comments::unix_line_breaks;
use crate::format::{FormatResult, format, format_source};

/// Format, then check that the output re-parses to an equivalent AST, keeps
/// every comment's text and place among the tokens, and is idempotent.
///
/// This doesn't compile the output for any target VM. Input that fails to
/// parse returns `Ok` with its errors in `FormatResult::errors`, as with
/// [`format()`].
///
/// It costs about two formats and three parses, plus two lexes when either
/// text has comments.
///
/// # Errors
///
/// Returns `Err((formatted, diff))` when a check fails.
pub fn format_and_verify(
    source: &str,
    version: LuaVersion,
    options: &FormatOptions,
) -> Result<FormatResult, (FormatResult, AstDiff)> {
    // Compare against the text the formatter read, not `source`, since
    // `sort_requires` may have reordered it.
    let (result, parsed) = format_source(source, version, options);
    let Some(original) = parsed else {
        return Ok(result);
    };
    let reformatted = luck_parser::parse(&result.output, version);
    if !reformatted.errors.is_empty() {
        let diff = AstDiff::new(
            "<root>",
            format!(
                "formatted output failed to re-parse: {}",
                reformatted.errors.first().map_or("<unknown>", |error| error.message.as_str())
            ),
        );
        return Err((result, diff));
    }
    if let Err(diff) = blocks_equivalent(&original.block, &reformatted.block, version) {
        return Err((result, diff));
    }
    // Two comment-free texts trivially agree, so skip the lexes.
    let has_comments = !original.comments.is_empty() || !reformatted.comments.is_empty();
    if has_comments && !comments_kept_in_place(&original.source, &result.output, version) {
        return Err((
            result,
            AstDiff::new(
                "<comments>",
                "formatting changed comment text or its position among tokens",
            ),
        ));
    }
    // Go through `format`, not `format_block` on the parse above. Only
    // `format` runs `sort_requires`, and skipping it would hide a sort that
    // reorders again on its second run.
    let second_output = format(&result.output, version, options).output;
    if second_output != result.output {
        return Err((result, AstDiff::new("<format>", "formatting is not idempotent")));
    }
    Ok(result)
}

/// Whether every comment of `original` survives in `formatted` with the same
/// text, between the same two tokens.
///
/// Both texts parse to the same AST but not the same tokens. The formatter
/// writes `;` and `,` as it sees fit (a broken table gains a trailing comma,
/// and a `;` field separator becomes one), and a claimed list prints a
/// comment after an item's separator even when the source had it before, so
/// separators are left out of the comparison. Grouping parens come and go
/// too (`f"s"` gains them under `call_parentheses = "always"`, and a condition
/// loses them), so a pair only one side has is skipped. A pair both sides
/// keep is a real boundary, which catches a comment pulled through a `(`.
fn comments_kept_in_place(original: &str, formatted: &str, version: LuaVersion) -> bool {
    let before = luck_lexer::lex(original, version);
    let after = luck_lexer::lex(formatted, version);
    let before_tokens = significant_tokens(&before.tokens);
    let after_tokens = significant_tokens(&after.tokens);
    let (before_ends, after_ends) = aligned_token_ends(&before_tokens, &after_tokens);
    comment_positions(original, &before_ends, &before.comments, version)
        == comment_positions(formatted, &after_ends, &after.comments, version)
}

/// The tokens a comment can be positioned against: everything but the
/// separators the formatter owns and the end marker.
fn significant_tokens(tokens: &[Token]) -> Vec<&Token> {
    tokens
        .iter()
        .filter(|token| {
            !matches!(token.kind, TokenKind::Semicolon | TokenKind::Comma | TokenKind::Eof)
        })
        .collect()
}

/// The span ends of the tokens present in both streams, one list per stream.
///
/// The walk goes in lockstep and skips a paren only one side has. Any other
/// mismatch ends it, and every comment past that point then counts as
/// following the last aligned token on both sides, so only its text and
/// order are still compared.
fn aligned_token_ends(left: &[&Token], right: &[&Token]) -> (Vec<u32>, Vec<u32>) {
    let is_paren =
        |token: &Token| matches!(token.kind, TokenKind::LeftParen | TokenKind::RightParen);

    let mut left_ends = Vec::with_capacity(left.len());
    let mut right_ends = Vec::with_capacity(right.len());
    let (mut left_pending, mut right_pending) = (left, right);
    while let ([left_token, left_rest @ ..], [right_token, right_rest @ ..]) =
        (left_pending, right_pending)
    {
        if discriminant(&left_token.kind) == discriminant(&right_token.kind) {
            left_ends.push(left_token.span.end);
            right_ends.push(right_token.span.end);
            left_pending = left_rest;
            right_pending = right_rest;
        } else if is_paren(left_token) {
            left_pending = left_rest;
        } else if is_paren(right_token) {
            right_pending = right_rest;
        } else {
            break;
        }
    }
    (left_ends, right_ends)
}

/// Each comment's text, keyed by how many aligned tokens precede it.
fn comment_positions<'source>(
    source: &'source str,
    aligned_ends: &[u32],
    comments: &[Comment],
    version: LuaVersion,
) -> Vec<(usize, Cow<'source, str>)> {
    comments
        .iter()
        .map(|comment| {
            let preceding_count = aligned_ends.partition_point(|&end| end <= comment.span.start);
            (preceding_count, unix_line_breaks(comment.span.text(source).trim_end(), version))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_check_sees_a_relocation() {
        let version = LuaVersion::Lua54;
        let before = "if a --[[ condition ]] then run() end";
        assert!(!comments_kept_in_place(before, "if a then run() end --[[ condition ]]", version));
        assert!(!comments_kept_in_place(before, "if a then run() end", version));
        // Parens only one side has and separators the formatter wrote aren't
        // boundaries to measure the comment against.
        assert!(comments_kept_in_place(
            before,
            "if (a) --[[ condition ]] then\n run();\nend",
            version
        ));
    }

    #[test]
    fn placement_check_sees_a_comment_pulled_through_a_kept_paren() {
        // Both texts keep the call's parens, so moving the comment inside
        // them puts it between a different pair of tokens.
        assert!(!comments_kept_in_place(
            "render --[[ why ]] (a, b)",
            "render(\n\t--[[ why ]]\n\ta,\n\tb\n)",
            LuaVersion::Lua54
        ));
    }

    #[test]
    fn placement_check_allows_the_parens_the_formatter_owns() {
        // `call_parentheses = "always"` wraps a bare table argument; the
        // comment inside it hasn't moved relative to any kept token.
        assert!(comments_kept_in_place(
            "render{a --[[ why ]]}",
            "render({ a --[[ why ]] })",
            LuaVersion::Lua54
        ));
    }
}
