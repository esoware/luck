use core::ops::Range;

use luck_ast::Block;
use luck_token::{LuaVersion, index_to_offset, offset_to_index};

use crate::comments::{Comments, write_remaining_comments, write_shebang};
use crate::format_block::{first_item_start, format_selection};
use crate::ir::{Format as _, Formatter};
use crate::sort_requires::sort_requires_in_source;
use crate::{FormatOptions, LineEndings};

#[derive(Debug, Clone)]
pub struct FormatResult {
    pub output: String,
    pub errors: Vec<FormatError>,
}

/// Formatting itself reports no errors.
pub type FormatError = luck_token::SourceError;

fn print_block(block: &Block, comments: Comments, options: &FormatOptions) -> String {
    let mut f = Formatter::new(*options, comments);
    write_shebang(&mut f);
    block.fmt(&mut f);
    // Empty and error statements print nothing, so a block holding only
    // those must not open a line before the trailing comment flush.
    let has_stmts = first_item_start(block).is_some();
    write_remaining_comments(&mut f, has_stmts);
    let mut output = f.print("");

    let trimmed = output.trim_end();
    output.truncate(trimmed.len());
    output.push('\n');
    if options.line_endings == LineEndings::Windows {
        output = with_windows_line_endings(&output);
    }
    output
}

/// Verbatim text can already hold CRLF or LFCR, from CRLF input or an earlier
/// pass, and a second CR would be another line break in a long string.
fn with_windows_line_endings(text: &str) -> String {
    let mut converted =
        String::with_capacity(text.len().saturating_add(text.matches('\n').count()));
    let mut segments = text.split_inclusive('\n').peekable();
    while let Some(segment) = segments.next() {
        let is_half_of_a_line_break =
            segment.ends_with("\r\n") || segments.peek().is_some_and(|next| next.starts_with('\r'));
        match segment.strip_suffix('\n') {
            Some(line) if !is_half_of_a_line_break => {
                converted.push_str(line);
                converted.push_str("\r\n");
            }
            Some(_) | None => converted.push_str(segment),
        }
    }
    converted
}

/// Trees built with `luck_ast::Synth` work as they are.
///
/// Pass [`Comments::synthetic`] to attach generated comments (chain [`Comments::with_blank_before`]
/// for blank lines between statements), [`Comments::from_source`] to keep a parsed file's comments
/// and blank lines, or [`Comments::none`].
#[must_use]
pub fn format_block(block: &Block, comments: Comments, options: &FormatOptions) -> String {
    print_block(block, comments, options)
}

/// Input that fails to parse returns empty output with its parse errors in
/// `FormatResult::errors`.
#[must_use]
pub fn format(source: &str, version: LuaVersion, options: &FormatOptions) -> FormatResult {
    format_source(source, version, options).0
}

/// Format only the statements that overlap a byte range, or those of the
/// innermost nested block holding the whole range.
///
/// Every byte outside them is copied as it was, including line endings and
/// the end of the file, which is what an editor's "format selection" wants.
/// `sort_requires` doesn't run in this mode.
#[must_use]
pub fn format_range(
    source: &str,
    version: LuaVersion,
    options: &FormatOptions,
    range: Range<usize>,
) -> FormatResult {
    let parsed = luck_parser::parse(source, version);
    if !parsed.errors.is_empty() {
        return FormatResult { output: String::new(), errors: parsed.errors };
    }

    let offsets = index_to_offset(range.start)..index_to_offset(range.end);
    let comments = Comments::from_source(&parsed.comments, source, version);
    let output = format_selection(&parsed.block, comments, options, &offsets).map_or_else(
        || source.to_owned(),
        |(span, selected)| {
            let text = if options.line_endings == LineEndings::Windows {
                with_windows_line_endings(&selected)
            } else {
                selected
            };
            let before = source.get(..offset_to_index(span.start)).unwrap_or_default();
            let after = source.get(offset_to_index(span.end)..).unwrap_or_default();
            let mut spliced = String::with_capacity(source.len());
            spliced.push_str(before);
            spliced.push_str(&text);
            spliced.push_str(after);
            spliced
        },
    );
    FormatResult { output, errors: Vec::new() }
}

/// Returns the parse the output came from too, so verification needn't
/// repeat it.
///
/// When `sort_requires` reordered the text, it's the parse of the
/// sorted text. Input that fails to parse returns only errors and no parse.
pub(crate) fn format_source(
    source: &str,
    version: LuaVersion,
    options: &FormatOptions,
) -> (FormatResult, Option<luck_parser::ParseResult>) {
    let source_parse = luck_parser::parse(source, version);
    if !source_parse.errors.is_empty() {
        let result = FormatResult { output: String::new(), errors: source_parse.errors };
        return (result, None);
    }

    let comments = Comments::from_source(&source_parse.comments, &source_parse.source, version);
    let mut output = print_block(&source_parse.block, comments, options);
    let mut parsed = source_parse;
    // `sort_requires` reorders source lines rather than AST nodes, so each
    // statement's comments and trivia move with it. It reads formatted text
    // so what it sees doesn't change on a second pass, and leaves the spans
    // stale, so the sorted text is parsed and printed again.
    if options.sort_requires {
        let formatted = luck_parser::parse(output.as_str(), version);
        if let Some(sorted) =
            sort_requires_in_source(&output, &formatted.block, &formatted.comments, version)
        {
            parsed = luck_parser::parse(sorted, version);
            let sorted_comments = Comments::from_source(&parsed.comments, &parsed.source, version);
            output = print_block(&parsed.block, sorted_comments, options);
        }
    }
    let result = FormatResult { output, errors: Vec::new() };
    (result, Some(parsed))
}
