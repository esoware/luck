//! # luck_formatter
//!
//! Prettier-style code formatter for Lua 5.1-5.5 and Luau.
//!
//! The primary entry point is [`format_block`], which formats an AST
//! directly - including programmatically constructed ASTs with dummy spans
//! (decompiler output). [`format`] and [`format_range`] are thin wrappers
//! that parse source text first; they additionally get source-fidelity
//! features (blank-line preservation, `-- luck: format off` verbatim
//! regions, comment attachment by byte position).
//!
//! # Usage
//!
//! ```
//! use luck_token::LuaVersion;
//! use luck_formatter::FormatOptions;
//!
//! let result = luck_formatter::format("local x=1", LuaVersion::Lua54, &FormatOptions::default());
//! assert!(result.errors.is_empty());
//! assert!(result.output.contains("local x = 1"));
//! ```

pub(crate) mod ast_equiv;
pub mod comments;
pub(crate) mod format_block;
pub(crate) mod format_expr;
pub(crate) mod format_function;
pub(crate) mod format_stmt;
pub(crate) mod format_table;
pub(crate) mod format_type;
pub mod ir;
pub(crate) mod numbers;
mod printer;
pub(crate) mod quotes;
pub mod sort_requires;
pub(crate) mod tokens;

use luck_ast::Block;
use luck_token::LuaVersion;

pub use ast_equiv::{AstDiff, blocks_equiv};
pub use comments::Comments;
use ir::Format;

// The format-option enums live in `luck_core` so config parsing can deserialize
// directly into them. Re-exported here so existing `luck_formatter::Xxx` paths
// keep resolving.
pub use luck_core::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, HexCase, IndentStyle, LineEndings,
    QuoteStyle, SpaceAfterFunction,
};

#[derive(Debug, Clone)]
pub struct FormatOptions {
    pub line_width: u16,
    pub indent_style: IndentStyle,
    pub indent_width: u8,
    pub quote_style: QuoteStyle,
    pub hexadecimal_case: HexCase,
    pub call_parentheses: CallParentheses,
    pub collapse_simple_statement: CollapseSimpleStatement,
    pub line_endings: LineEndings,
    pub block_newline_gaps: BlockNewlineGaps,
    pub sort_requires: bool,
    pub space_after_function_names: SpaceAfterFunction,
    /// When true, a trailing comma in a table or call argument list forces
    /// the surrounding group to break across multiple lines (Black/Prettier style).
    pub magic_trailing_comma: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            line_width: 100,
            indent_style: IndentStyle::Tabs,
            indent_width: 4,
            quote_style: QuoteStyle::Double,
            hexadecimal_case: HexCase::Preserve,
            call_parentheses: CallParentheses::Always,
            collapse_simple_statement: CollapseSimpleStatement::Never,
            line_endings: LineEndings::default(),
            block_newline_gaps: BlockNewlineGaps::default(),
            sort_requires: false,
            space_after_function_names: SpaceAfterFunction::default(),
            // Default false to preserve existing fill/hug behavior; users opt in
            // for Black/Prettier semantics. (We pick false rather than true so
            // that existing fixtures with trailing commas keep their packed
            // layout - flipping a default mid-release breaks downstreams.)
            magic_trailing_comma: false,
        }
    }
}

/// The single mapping from a config `FormatConfig` to `FormatOptions`.
///
/// Starts from `FormatOptions::default()` and applies each `Some(..)` field,
/// leaving the default in place where the config is `None`. Both the CLI and
/// the LSP route through this so they agree on every field.
impl From<&luck_core::config::FormatConfig> for FormatOptions {
    fn from(config: &luck_core::config::FormatConfig) -> Self {
        let mut options = FormatOptions::default();
        if let Some(width) = config.line_width {
            options.line_width = width;
        }
        if let Some(indent_style) = config.indent_style {
            options.indent_style = indent_style;
        }
        if let Some(width) = config.indent_width {
            options.indent_width = width;
        }
        if let Some(quote_style) = config.quote_style {
            options.quote_style = quote_style;
        }
        if let Some(hexadecimal_case) = config.hexadecimal_case {
            options.hexadecimal_case = hexadecimal_case;
        }
        if let Some(call_parentheses) = config.call_parentheses {
            options.call_parentheses = call_parentheses;
        }
        if let Some(collapse) = config.collapse_simple_statement {
            options.collapse_simple_statement = collapse;
        }
        if let Some(line_endings) = config.line_endings {
            options.line_endings = line_endings;
        }
        if let Some(gaps) = config.block_newline_gaps {
            options.block_newline_gaps = gaps;
        }
        if let Some(sort_requires) = config.sort_requires {
            options.sort_requires = sort_requires;
        }
        if let Some(space) = config.space_after_function_names {
            options.space_after_function_names = space;
        }
        if let Some(magic_trailing_comma) = config.magic_trailing_comma {
            options.magic_trailing_comma = magic_trailing_comma;
        }
        options
    }
}

#[derive(Debug, Clone)]
pub struct FormatResult {
    pub output: String,
    pub errors: Vec<FormatError>,
}

/// A formatter error with position and message.
pub type FormatError = luck_token::SourceError;

/// Run the shared IR -> text pipeline for an already-configured formatter.
fn run_pipeline(block: &Block, mut formatter: ir::Formatter) -> String {
    formatter.emit_shebang();
    block.fmt(&mut formatter);
    let has_statements = !block.stmts.is_empty() || block.last_stmt.is_some();
    formatter.emit_remaining_comments(has_statements);

    let printer_options = printer::PrinterOptions {
        line_width: formatter.options.line_width,
        use_tabs: formatter.options.indent_style == IndentStyle::Tabs,
        indent_width: formatter.options.indent_width,
    };
    let line_endings = formatter.options.line_endings;
    let group_count = formatter.group_count();
    let elements = formatter.into_elements();
    let mut output = printer::print(&elements, group_count, &printer_options);

    let trimmed = output.trim_end();
    output.truncate(trimmed.len());
    output.push('\n');
    if line_endings == LineEndings::Windows {
        output = output.replace('\n', "\r\n");
    }
    output
}

/// Format an AST directly - the primary entry point.
///
/// Works on any AST, including programmatically constructed ones with dummy
/// spans (see `luck_ast::synth`). Pass [`Comments::synthetic`] to attach
/// generated comments (chain [`Comments::with_blank_before`] to request
/// blank lines between statements), [`Comments::from_source`] when the AST
/// came from real source and comment/blank-line fidelity matters, or
/// [`Comments::none`].
#[must_use]
pub fn format_block(block: &Block, comments: Comments, options: &FormatOptions) -> String {
    let formatter = ir::Formatter::with_context(options.clone(), comments);
    run_pipeline(block, formatter)
}

/// Format Lua source code according to the given options.
///
/// Parses the source, and if parsing succeeds, returns formatted output.
/// Parse errors are collected into `FormatResult::errors`.
#[must_use]
pub fn format(source: &str, version: LuaVersion, options: &FormatOptions) -> FormatResult {
    format_source(source, version, options, None)
}

/// Format only the statements overlapping with a byte range.
///
/// Parses the entire file but only formats statements within the range,
/// emitting the rest verbatim from source. Useful for editor "format selection".
#[must_use]
pub fn format_range(
    source: &str,
    version: LuaVersion,
    options: &FormatOptions,
    range: std::ops::Range<usize>,
) -> FormatResult {
    format_source(
        source,
        version,
        options,
        Some(range.start as u32..range.end as u32),
    )
}

fn format_source(
    source: &str,
    version: LuaVersion,
    options: &FormatOptions,
    format_range: Option<std::ops::Range<u32>>,
) -> FormatResult {
    let parse_result = luck_parser::parse(source, version);

    let errors: Vec<FormatError> = parse_result
        .errors
        .iter()
        .map(|err| FormatError {
            span: err.span,
            message: err.message.clone(),
        })
        .collect();
    if !errors.is_empty() {
        return FormatResult {
            output: String::new(),
            errors,
        };
    }

    // sort_requires runs as a source-level pre-pass before IR construction:
    // reordering statements after IR is built would invalidate captured spans.
    let owned_source;
    let working_source: &str = if options.sort_requires && format_range.is_none() {
        owned_source = sort_requires::sort_requires_in_source(
            source,
            &parse_result.block,
            &parse_result.comments,
        );
        &owned_source
    } else {
        source
    };

    // If we rewrote, re-parse so spans match the new buffer.
    let parse_result = if std::ptr::eq(working_source.as_ptr(), source.as_ptr()) {
        parse_result
    } else {
        luck_parser::parse(working_source, version)
    };

    let comments = Comments::from_source(&parse_result.comments, working_source);
    let mut formatter = ir::Formatter::with_context(options.clone(), comments);
    formatter.format_range = format_range;

    FormatResult {
        output: run_pipeline(&parse_result.block, formatter),
        errors: vec![],
    }
}

/// Verify that formatting was structure-preserving by re-parsing the output
/// and comparing the new block to the original.
///
/// Returns `Ok(formatted)` when the AST is equivalent, or `Err((formatted, diff))`
/// otherwise so callers can show a diagnostic.
pub fn format_and_verify(
    source: &str,
    version: LuaVersion,
    options: &FormatOptions,
) -> Result<FormatResult, (FormatResult, AstDiff)> {
    let result = format(source, version, options);
    if !result.errors.is_empty() {
        return Ok(result);
    }
    let original = luck_parser::parse(source, version);
    let reformatted = luck_parser::parse(&result.output, version);
    if !reformatted.errors.is_empty() {
        let diff = AstDiff {
            path: "<root>".to_string(),
            reason: format!(
                "formatted output failed to re-parse: {}",
                reformatted
                    .errors
                    .first()
                    .map(|e| e.message.as_str())
                    .unwrap_or("<unknown>")
            ),
        };
        return Err((result, diff));
    }
    match blocks_equiv(&original.block, &reformatted.block) {
        Ok(()) => Ok(result),
        Err(diff) => Err((result, diff)),
    }
}
