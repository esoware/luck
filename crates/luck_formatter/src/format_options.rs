use luck_core::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatConfig, HexCase, IndentStyle,
    LineEndings, QuoteStyle, SpaceAfterFunction,
};

#[derive(Debug, Clone, Copy)]
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
    /// When true, a trailing comma in a table constructor breaks the table one
    /// field per line, as in Black and Prettier.
    ///
    /// Argument lists can't carry one, since no dialect allows a comma after
    /// the last argument.
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
            magic_trailing_comma: false,
        }
    }
}

/// Applies each `Some` field of the config over `FormatOptions::default()`.
///
/// The CLI and the LSP both convert through this, so they agree on every
/// field.
impl From<&FormatConfig> for FormatOptions {
    fn from(config: &FormatConfig) -> Self {
        let mut options = Self::default();
        if let Some(line_width) = config.line_width {
            options.line_width = line_width;
        }
        if let Some(indent_style) = config.indent_style {
            options.indent_style = indent_style;
        }
        if let Some(indent_width) = config.indent_width {
            options.indent_width = indent_width;
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
        if let Some(collapse_simple_statement) = config.collapse_simple_statement {
            options.collapse_simple_statement = collapse_simple_statement;
        }
        if let Some(line_endings) = config.line_endings {
            options.line_endings = line_endings;
        }
        if let Some(block_newline_gaps) = config.block_newline_gaps {
            options.block_newline_gaps = block_newline_gaps;
        }
        if let Some(sort_requires) = config.sort_requires {
            options.sort_requires = sort_requires;
        }
        if let Some(space_after_function_names) = config.space_after_function_names {
            options.space_after_function_names = space_after_function_names;
        }
        if let Some(magic_trailing_comma) = config.magic_trailing_comma {
            options.magic_trailing_comma = magic_trailing_comma;
        }
        options
    }
}
