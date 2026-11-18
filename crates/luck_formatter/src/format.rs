use core::ops::Range;

use luck_allocator::Allocator;
use luck_ast::{Block, MAX_NESTING_DEPTH, block_nesting_depth};
use luck_parser::ParseResult;
use luck_token::{LuaVersion, index_to_offset, offset_to_index};

use crate::comments::Comments;
use crate::format_block::{first_item_start, format_selection};
use crate::ir::Formatter;
use crate::{FormatOptions, LineEndings};

#[derive(Debug, Clone)]
pub struct FormatResult {
    pub output: String,
    pub errors: Vec<FormatError>,
}

/// A parse error, or the refusal of a tree nested past the cap. Formatting
/// itself reports no others.
pub type FormatError = luck_token::SourceError;

fn print_block<'ast>(
    allocator: &'ast Allocator,
    block: &'ast Block<'ast>,
    comments: Comments<'ast>,
    options: &FormatOptions,
) -> String {
    let mut f = Formatter::new(allocator, *options, comments);
    if options.sort_requires {
        f = f.sorting_requires();
    }
    f.write_shebang();
    f.write_block(block);
    // Empty and error statements print nothing, so a block holding only
    // those must not open a line before the trailing comment flush.
    let has_stmts = first_item_start(block).is_some();
    f.write_remaining_comments(has_stmts);
    let mut output = f.print("");

    let trimmed = output.trim_end();
    output.truncate(trimmed.len());
    output.push_str(match options.line_endings {
        LineEndings::Unix => "\n",
        LineEndings::Windows => "\r\n",
    });
    output
}

/// Formats a tree, including one built in code with no source text, such as
/// with `luck_ast::Synth`.
///
/// Pass [`Comments::synthetic`] to attach generated comments (chain
/// [`Comments::with_blank_before`] for blank lines between statements), or
/// [`Comments::none`]. Text the formatter rewrites, such as a requoted
/// string, goes in `allocator`.
///
/// # Errors
///
/// Returns an error for a tree nested deeper than [`MAX_NESTING_DEPTH`],
/// which `luck_parser` never builds and would refuse to read back.
pub fn format_block<'ast>(
    allocator: &'ast Allocator,
    block: &'ast Block<'ast>,
    comments: Comments<'ast>,
    options: &FormatOptions,
) -> Result<String, FormatError> {
    if block_nesting_depth(block) > MAX_NESTING_DEPTH {
        return Err(FormatError {
            span: block.span,
            message: format!(
                "maximum nesting depth exceeded (luck resource limit: {MAX_NESTING_DEPTH})"
            ),
        });
    }
    Ok(print_block(allocator, block, comments, options))
}

/// Parses `source` into `allocator` and formats it. Input that fails to parse
/// returns empty output with its parse errors in `FormatResult::errors`.
#[must_use]
pub fn format(
    allocator: &Allocator,
    source: &str,
    version: LuaVersion,
    options: &FormatOptions,
) -> FormatResult {
    let parse = luck_parser::parse(allocator, source, version);
    format_parsed(allocator, &parse, version, options)
}

/// [`format()`] without the parse, for a host that already holds one.
///
/// `parse` must come from `luck_parser` at `version`. One with errors returns
/// empty output and those errors. Text the formatter rewrites goes in
/// `allocator`, which may be the one the tree lives in.
#[must_use]
pub fn format_parsed<'ast>(
    allocator: &'ast Allocator,
    parse: &'ast ParseResult<'ast>,
    version: LuaVersion,
    options: &FormatOptions,
) -> FormatResult {
    if !parse.errors.is_empty() {
        return FormatResult { output: String::new(), errors: parse.errors.clone() };
    }
    let comments = Comments::from_source(&parse.comments, parse.source, version);
    let output = print_block(allocator, &parse.block, comments, options);
    FormatResult { output, errors: Vec::new() }
}

/// Format only the statements that overlap a byte range, or those of the
/// innermost nested block holding the whole range.
///
/// Every byte outside them is copied as it was, including line endings and
/// the end of the file, which is what an editor's "format selection" wants.
/// It never sorts requires, since a selection can split a run of them.
/// `parse` is as for [`format_parsed`].
#[must_use]
pub fn format_range<'ast>(
    allocator: &'ast Allocator,
    parse: &'ast ParseResult<'ast>,
    version: LuaVersion,
    options: &FormatOptions,
    range: Range<usize>,
) -> FormatResult {
    if !parse.errors.is_empty() {
        return FormatResult { output: String::new(), errors: parse.errors.clone() };
    }
    let source = parse.source;
    let offsets = index_to_offset(range.start)..index_to_offset(range.end);
    let comments = Comments::from_source(&parse.comments, source, version);
    let output = format_selection(allocator, &parse.block, comments, options, &offsets)
        .map_or_else(
            || source.to_owned(),
            |(span, selected)| {
                let before = source.get(..offset_to_index(span.start)).unwrap_or_default();
                let after = source.get(offset_to_index(span.end)..).unwrap_or_default();
                let mut spliced = String::with_capacity(source.len());
                spliced.push_str(before);
                spliced.push_str(&selected);
                spliced.push_str(after);
                spliced
            },
        );
    FormatResult { output, errors: Vec::new() }
}
