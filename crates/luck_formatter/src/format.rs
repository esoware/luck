use core::ops::Range;

use luck_allocator::Allocator;
use luck_ast::{Chunk, MAX_NESTING_DEPTH, block_nesting_depth};
use luck_diagnostics::Diagnostic;
use luck_parser::{Groupings, ParseResult};
use luck_token::{LuaVersion, index_to_offset, is_lua_whitespace, offset_to_index};

use crate::comments::Comments;
use crate::format_block::{first_item_start, format_selection};
use crate::ir::Formatter;
use crate::{FormatOptions, LineEndings};

#[derive(Debug, Clone)]
pub struct FormatResult {
    /// Bytes, as the source is, which are UTF-8 when the source is.
    pub output: Vec<u8>,
    /// The parse's errors, each an E008. Formatting itself reports none.
    pub errors: Vec<Diagnostic>,
}

fn print_chunk<'ast>(
    allocator: &'ast Allocator,
    chunk: &'ast Chunk<'ast>,
    comments: Comments<'ast>,
    groupings: Option<&'ast Groupings>,
    options: &FormatOptions,
) -> Vec<u8> {
    let mut f = Formatter::new(allocator, chunk.version, *options, comments);
    if let Some(parse_groupings) = groupings {
        f = f.with_groupings(parse_groupings);
    }
    if options.sort_requires {
        f = f.sorting_requires();
    }
    f.write_shebang();
    f.write_block(&chunk.block);
    // Empty and error statements print nothing, so a block holding only
    // those must not open a line before the trailing comment flush.
    let has_stmts = first_item_start(&chunk.block).is_some();
    f.write_remaining_comments(has_stmts);
    let mut output = f.print(b"");

    let trimmed_length = output
        .iter()
        .rposition(|&b| !is_lua_whitespace(b))
        .map_or(0, |last| last.saturating_add(1));
    output.truncate(trimmed_length);
    output.extend_from_slice(match options.line_endings {
        LineEndings::Unix => b"\n",
        LineEndings::Windows => b"\r\n",
    });
    output
}

/// Formats a tree, including one built in code with no source text, such as
/// with `luck_ast::Synth`, whose literals it spells for the chunk's version.
///
/// Pass [`Comments::synthetic`] to attach generated comments (chain
/// [`Comments::with_blank_before`] for blank lines between statements), or
/// [`Comments::none`]. Text the formatter rewrites, such as a requoted
/// string, goes in `allocator`.
///
/// # Errors
///
/// Returns an E008 for a tree nested deeper than [`MAX_NESTING_DEPTH`],
/// which `luck_parser` never builds and would refuse to read back.
pub fn format_chunk<'ast>(
    allocator: &'ast Allocator,
    chunk: &'ast Chunk<'ast>,
    comments: Comments<'ast>,
    options: &FormatOptions,
) -> Result<Vec<u8>, Diagnostic> {
    if block_nesting_depth(&chunk.block) > MAX_NESTING_DEPTH {
        return Err(Diagnostic::e008(
            chunk.block.span,
            format!("maximum nesting depth exceeded (luck resource limit: {MAX_NESTING_DEPTH})"),
        ));
    }
    Ok(print_chunk(allocator, chunk, comments, None, options))
}

/// Parses `source` into `allocator` and formats it. Input that fails to parse
/// returns empty output with its parse errors in `FormatResult::errors`.
///
/// `source` is bytes, as Lua reads it, and the formatter copies a comment or
/// a string that isn't UTF-8 as written.
#[must_use]
pub fn format(
    allocator: &Allocator,
    source: &[u8],
    version: LuaVersion,
    options: &FormatOptions,
) -> FormatResult {
    let parse = luck_parser::parse_with_tokens(allocator, source, version);
    format_parsed(allocator, &parse, options)
}

/// [`format()`] without the parse, for a host that already holds one, made
/// with `luck_parser::parse_with_tokens`: the formatter finds the tokens the
/// tree keeps no span for among its tokens.
///
/// A parse with errors returns empty output and those errors. Text the
/// formatter rewrites goes in `allocator`, which may be the one the tree
/// lives in.
///
/// # Panics
///
/// In debug builds, when `parse` kept no tokens, as `luck_parser::parse`
/// keeps none.
#[must_use]
pub fn format_parsed<'ast>(
    allocator: &'ast Allocator,
    parse: &'ast ParseResult<'ast>,
    options: &FormatOptions,
) -> FormatResult {
    if !parse.errors.is_empty() {
        return FormatResult { output: Vec::new(), errors: parse.errors.clone() };
    }
    let comments = Comments::from_parse(parse);
    let output = print_chunk(allocator, &parse.chunk, comments, Some(&parse.groupings), options);
    FormatResult { output, errors: Vec::new() }
}

/// Format only the statements that overlap a byte range, or those of the
/// innermost nested block holding the whole range.
///
/// Every byte outside them is copied as it was, including line endings and
/// the end of the file, which is what an editor's "format selection" wants.
/// It never sorts requires, since a selection can split a run of them.
/// `parse` is as for [`format_parsed`].
///
/// # Panics
///
/// As [`format_parsed`] does.
#[must_use]
pub fn format_range<'ast>(
    allocator: &'ast Allocator,
    parse: &'ast ParseResult<'ast>,
    options: &FormatOptions,
    range: Range<usize>,
) -> FormatResult {
    if !parse.errors.is_empty() {
        return FormatResult { output: Vec::new(), errors: parse.errors.clone() };
    }
    let source = parse.source;
    let offsets = index_to_offset(range.start)..index_to_offset(range.end);
    let comments = Comments::from_parse(parse);
    let output = format_selection(allocator, parse, comments, options, &offsets).map_or_else(
        || source.to_owned(),
        |(span, selected)| {
            let before = source.get(..offset_to_index(span.start)).unwrap_or_default();
            let after = source.get(offset_to_index(span.end)..).unwrap_or_default();
            [before, &selected, after].concat()
        },
    );
    FormatResult { output, errors: Vec::new() }
}
