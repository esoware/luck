use core::ops::Range;

use luck_ast::{Block, Call, FunctionArgs, MAX_NESTING_DEPTH, Visitor, block_nesting_depth};
use luck_parser::ParseResult;
use luck_token::{LuaVersion, index_to_offset, offset_to_index};

use crate::comments::{Comments, write_remaining_comments, write_shebang};
use crate::format_block::{first_item_start, format_selection};
use crate::ir::{Format as _, Formatter};
use crate::{CallParentheses, FormatOptions, LineEndings};

#[derive(Debug, Clone)]
pub struct FormatResult {
    pub output: String,
    pub errors: Vec<FormatError>,
}

/// A parse error, or the refusal of a tree nested past the cap. Formatting
/// itself reports no others.
pub type FormatError = luck_token::SourceError;

fn print_block(
    block: &Block,
    comments: Comments,
    options: &FormatOptions,
    should_sort_requires: bool,
) -> String {
    let mut f = Formatter::new(*options, comments);
    if should_sort_requires {
        f = f.sorting_requires();
    }
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

/// `options`, or ones that keep every call's arguments as they are.
///
/// The parentheses `options` put around a bare table or string argument are
/// a level each, which can nest the output of `block`, `depth` levels deep,
/// past `limit`.
fn fitting_options(
    block: &Block,
    depth: usize,
    limit: usize,
    options: &FormatOptions,
) -> FormatOptions {
    // The parentheses add at most a level for each level of the tree.
    if depth.saturating_mul(2) <= limit || printed_depth(block, depth, options) <= limit {
        return *options;
    }
    FormatOptions { call_parentheses: CallParentheses::Input, ..*options }
}

fn printed_depth(block: &Block, depth: usize, options: &FormatOptions) -> usize {
    let (has_table_parens, has_string_parens) = match options.call_parentheses {
        CallParentheses::Always => (true, true),
        CallParentheses::NoSingleString => (true, false),
        CallParentheses::NoSingleTable => (false, true),
        CallParentheses::None | CallParentheses::Input => (false, false),
    };
    let mut meter = PrintedDepth { depth: 0, deepest: 0, has_table_parens, has_string_parens };
    meter.visit_block(block);
    // The walk skips attribute arguments, which `depth` counts.
    meter.deepest.max(depth)
}

struct PrintedDepth {
    depth: usize,
    deepest: usize,
    has_table_parens: bool,
    has_string_parens: bool,
}

impl<'ast> Visitor<'ast> for PrintedDepth {
    fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        self.deepest = self.deepest.max(self.depth);
        self.depth <= MAX_NESTING_DEPTH
    }

    fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn visit_call(&mut self, call: Call<'ast>) {
        let gains_parentheses = match call.args {
            FunctionArgs::TableConstructor(_) => self.has_table_parens,
            FunctionArgs::StringLiteral(_) => self.has_string_parens,
            FunctionArgs::Parenthesized { .. } | FunctionArgs::Error(_) => false,
        };
        if !gains_parentheses {
            self.walk_call(call);
            return;
        }
        if self.descend() {
            self.walk_call(call);
        }
        self.ascend();
    }
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
///
/// # Errors
///
/// Returns an error for a tree nested deeper than [`MAX_NESTING_DEPTH`],
/// which `luck_parser` never builds and would refuse to read back.
pub fn format_block(
    block: &Block,
    comments: Comments,
    options: &FormatOptions,
) -> Result<String, FormatError> {
    let depth = block_nesting_depth(block);
    if depth > MAX_NESTING_DEPTH {
        return Err(FormatError {
            span: block.span,
            message: format!("maximum nesting depth exceeded (limit: {MAX_NESTING_DEPTH})"),
        });
    }
    let fitting = fitting_options(block, depth, MAX_NESTING_DEPTH, options);
    Ok(print_block(block, comments, &fitting, false))
}

/// Input that fails to parse returns empty output with its parse errors in
/// `FormatResult::errors`.
#[must_use]
pub fn format(source: &str, version: LuaVersion, options: &FormatOptions) -> FormatResult {
    format_parsed(&luck_parser::parse(source, version), version, options)
}

/// [`format()`] without the parse, for a host that already holds one.
///
/// `parse` must come from `luck_parser` at `version`. One with errors returns
/// empty output and those errors.
#[must_use]
pub fn format_parsed(
    parse: &ParseResult,
    version: LuaVersion,
    options: &FormatOptions,
) -> FormatResult {
    if !parse.errors.is_empty() {
        return FormatResult { output: String::new(), errors: parse.errors.clone() };
    }
    let comments = Comments::from_source(&parse.comments, &parse.source, version);
    let limit = version.max_nesting_depth();
    let fitting = fitting_options(&parse.block, parse.nesting_depth, limit, options);
    let output = print_block(&parse.block, comments, &fitting, fitting.sort_requires);
    FormatResult { output, errors: Vec::new() }
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
    let limit = version.max_nesting_depth();
    let fitting = fitting_options(&parsed.block, parsed.nesting_depth, limit, options);
    let output = format_selection(&parsed.block, comments, &fitting, &offsets).map_or_else(
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
