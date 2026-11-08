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
            line_endings: LineEndings::Unix,
            block_newline_gaps: BlockNewlineGaps::Never,
            sort_requires: false,
            space_after_function_names: SpaceAfterFunction::Never,
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
        // Destructured in full so a new config field can't go unmapped.
        let FormatConfig {
            line_width,
            indent_style,
            indent_width,
            quote_style,
            hexadecimal_case,
            call_parentheses,
            collapse_simple_statement,
            line_endings,
            block_newline_gaps,
            sort_requires,
            space_after_function_names,
            magic_trailing_comma,
        } = *config;
        let defaults = Self::default();
        Self {
            line_width: line_width.unwrap_or(defaults.line_width),
            indent_style: indent_style.unwrap_or(defaults.indent_style),
            indent_width: indent_width.unwrap_or(defaults.indent_width),
            quote_style: quote_style.unwrap_or(defaults.quote_style),
            hexadecimal_case: hexadecimal_case.unwrap_or(defaults.hexadecimal_case),
            call_parentheses: call_parentheses.unwrap_or(defaults.call_parentheses),
            collapse_simple_statement: collapse_simple_statement
                .unwrap_or(defaults.collapse_simple_statement),
            line_endings: line_endings.unwrap_or(defaults.line_endings),
            block_newline_gaps: block_newline_gaps.unwrap_or(defaults.block_newline_gaps),
            sort_requires: sort_requires.unwrap_or(defaults.sort_requires),
            space_after_function_names: space_after_function_names
                .unwrap_or(defaults.space_after_function_names),
            magic_trailing_comma: magic_trailing_comma.unwrap_or(defaults.magic_trailing_comma),
        }
    }
}
