use core::ops::Range;

use luck_ast::Block;
use luck_token::{LuaVersion, index_to_offset};

use crate::comments::{Comments, write_remaining_comments, write_shebang};
use crate::format_block::first_item_start;
use crate::ir::{Format as _, Formatter};
use crate::sort_requires::sort_requires_in_source;
use crate::{FormatOptions, LineEndings};

#[derive(Debug, Clone)]
pub struct FormatResult {
    pub output: String,
    pub errors: Vec<FormatError>,
}

/// A parse error in the input. Formatting itself reports no errors.
pub type FormatError = luck_token::SourceError;

/// Lay out `block` as text, the tail every entry point shares.
fn print_block(
    block: &Block,
    comments: Comments,
    options: &FormatOptions,
    format_range: Option<Range<u32>>,
) -> String {
    let mut formatter = Formatter::new(*options, comments, format_range);
    write_shebang(&mut formatter);
    block.fmt(&mut formatter);
    // Empty and error statements print nothing, so a block holding only
    // those must not open a line before the trailing comment flush.
    let has_stmts = first_item_start(block).is_some();
    write_remaining_comments(&mut formatter, has_stmts);
    let mut output = formatter.print();

    let trimmed = output.trim_end();
    output.truncate(trimmed.len());
    output.push('\n');
    if options.line_endings == LineEndings::Windows {
        output = output.replace('\n', "\r\n");
    }
    output
}

/// Format an AST, with or without source text behind it.
///
/// Trees built with `luck_ast::Synth` work as they are. Pass
/// [`Comments::synthetic`] to attach generated comments (chain
/// [`Comments::with_blank_before`] for blank lines between statements),
/// [`Comments::from_source`] to keep a parsed file's comments and blank lines,
/// or [`Comments::none`].
#[must_use]
pub fn format_block(block: &Block, comments: Comments, options: &FormatOptions) -> String {
    print_block(block, comments, options, None)
}

/// Format Lua source text.
///
/// Input that fails to parse returns empty output with its parse errors in
/// `FormatResult::errors`.
#[must_use]
pub fn format(source: &str, version: LuaVersion, options: &FormatOptions) -> FormatResult {
    format_source(source, version, options, None).0
}

/// Format only the statements that overlap a byte range.
///
/// The rest of the file is copied verbatim, which is what an editor's "format
/// selection" wants. `sort_requires` doesn't run in this mode.
#[must_use]
pub fn format_range(
    source: &str,
    version: LuaVersion,
    options: &FormatOptions,
    range: Range<usize>,
) -> FormatResult {
    format_source(
        source,
        version,
        options,
        Some(index_to_offset(range.start)..index_to_offset(range.end)),
    )
    .0
}

/// Shared body of [`format()`], [`format_range`], and
/// [`format_and_verify`](crate::format_and_verify).
///
/// Also returns the parse the output came from, so verification needn't
/// repeat it. Its `source` is the text the formatter actually read, which
/// `sort_requires` may have rewritten. Input that fails to parse returns only
/// errors and no parse.
pub(crate) fn format_source(
    source: &str,
    version: LuaVersion,
    options: &FormatOptions,
    format_range: Option<Range<u32>>,
) -> (FormatResult, Option<luck_parser::ParseResult>) {
    let source_parse = luck_parser::parse(source, version);
    if !source_parse.errors.is_empty() {
        let result = FormatResult { output: String::new(), errors: source_parse.errors };
        return (result, None);
    }

    // `sort_requires` reorders source lines rather than AST nodes, so each
    // statement's comments and trivia move with it. The rewrite leaves the
    // old spans stale, so the new text is parsed again.
    let parsed = if options.sort_requires
        && format_range.is_none()
        && let Some(sorted) =
            sort_requires_in_source(source, &source_parse.block, &source_parse.comments)
    {
        luck_parser::parse(sorted, version)
    } else {
        source_parse
    };

    let comments = Comments::from_source(&parsed.comments, &parsed.source, version);
    let output = print_block(&parsed.block, comments, options, format_range);
    let result = FormatResult { output, errors: Vec::new() };
    (result, Some(parsed))
}
