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
    /// When true, a trailing comma in a table constructor keeps the table
    /// broken, as in Black and Prettier: one field per line, or still packed
    /// when it's a list the source packed several to a line.
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
            sort_requires: true,
            space_after_function_names: SpaceAfterFunction::Never,
            magic_trailing_comma: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentStyle {
    Tabs,
    Spaces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteStyle {
    /// Always use double quotes.
    Double,
    /// Always use single quotes.
    Single,
    /// Use double quotes, switching to single when that needs strictly
    /// fewer escaped quotes.
    AutoPreferDouble,
    /// Use single quotes, switching to double when that needs strictly
    /// fewer escaped quotes.
    AutoPreferSingle,
}

/// Case of the hexadecimal digits `A`-`F` in numeric literals. The base
/// prefix (`0x`, `0b`) and exponent markers (`e`, `p`) are always lowercase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexCase {
    /// Leave the author's digit case untouched (`0xB0` and `0xb0` both stand).
    Preserve,
    /// `0xB0` -> `0xb0`.
    Lower,
    /// `0xb0` -> `0xB0`.
    Upper,
}

/// When a call drops the parentheses around a lone string or table argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallParentheses {
    /// Always use parentheses: `f("hello")`, `f({1, 2})`.
    Always,
    /// Omit parentheses for a single string argument: `f "hello"`.
    NoSingleString,
    /// Omit parentheses for a single table argument: `f {1, 2}`.
    NoSingleTable,
    /// Omit parentheses for single string or table arguments.
    None,
    /// Keep the source's choice: `f"x"` stays bare and `f("x")` keeps its
    /// parentheses.
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEndings {
    Unix,
    Windows,
}

/// Which simple single-statement blocks collapse onto one line. Function
/// bodies never do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollapseSimpleStatement {
    /// Always expand blocks.
    Never,
    /// Collapse simple conditionals: `if x then return y end`.
    ConditionalOnly,
}

/// Whether a block body can start or end with a blank line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockNewlineGaps {
    /// Strip blank lines at the start and end of every block body.
    Never,
    /// Keep one blank line at the start or end of a block body when the source
    /// has any there.
    Preserve,
}

/// Where a space goes before the `(` of a function definition or call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceAfterFunction {
    /// No space: `function foo()`, `foo()`.
    Never,
    /// Space before a definition's parameter list only: `function foo ()`.
    Definitions,
    /// Space between a callee and its `(` only: `foo ()`.
    Calls,
    /// Space in both definitions and calls.
    Always,
}
