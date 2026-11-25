//! The `format` section of `luck.json`. Each field is the [`FormatOptions`]
//! option of the same name, unset when the config leaves it out, so a section
//! layers over `.editorconfig` and over another section, and `FormatOptions`
//! holds the default it takes then.

use crate::format_options::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatOptions, HexCase,
    IndentStyle, LineEndings, QuoteStyle, SpaceAfterFunction,
};

/// Formatter settings from luck.json.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize), serde(deny_unknown_fields))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct FormatConfig {
    pub line_width: Option<u16>,
    pub indent_style: Option<IndentStyle>,
    /// Columns per indent level. With tabs, this is how wide a tab counts
    /// against `line_width`.
    pub indent_width: Option<u8>,
    pub quote_style: Option<QuoteStyle>,
    /// Case of the hex digits `A`-`F` in numeric literals: `"preserve"`
    /// (default), `"lower"`, or `"upper"`. The `0x` prefix is always
    /// lowercase.
    pub hexadecimal_case: Option<HexCase>,
    pub call_parentheses: Option<CallParentheses>,
    pub collapse_simple_statement: Option<CollapseSimpleStatement>,
    pub line_endings: Option<LineEndings>,
    /// Whether to keep a blank line at the start or end of a block body:
    /// `"never"` (default) or `"preserve"`.
    pub block_newline_gaps: Option<BlockNewlineGaps>,
    /// When true, sorts each run of consecutive `local NAME = require(...)`
    /// or `local NAME = game:GetService(...)` statements by name, where the
    /// require takes a string or a dotted path such as `script.Parent.Module`.
    /// A blank line ends a run. Defaults to `false`, because a module's body
    /// runs at its first `require`, so sorting can change what a program does.
    pub sort_requires: Option<bool>,
    /// Where to put a space before `(`: `"never"` (default), `"definitions"`,
    /// `"calls"`, or `"always"` for both.
    pub space_after_function_names: Option<SpaceAfterFunction>,
    /// When true, a trailing comma in a table constructor keeps the table
    /// broken, as in Black and Prettier: one field per line, or still packed
    /// when it's a list the source packed several to a line. Defaults to
    /// `true`. Argument lists can't carry one, since the grammar allows no
    /// comma after the last argument.
    pub magic_trailing_comma: Option<bool>,
}

impl FormatConfig {
    /// `self` layered on `base`: each option `self` sets, and else `base`'s.
    #[must_use]
    pub fn merge_onto(self, base: Self) -> Self {
        Self {
            line_width: self.line_width.or(base.line_width),
            indent_style: self.indent_style.or(base.indent_style),
            indent_width: self.indent_width.or(base.indent_width),
            quote_style: self.quote_style.or(base.quote_style),
            hexadecimal_case: self.hexadecimal_case.or(base.hexadecimal_case),
            call_parentheses: self.call_parentheses.or(base.call_parentheses),
            collapse_simple_statement: self
                .collapse_simple_statement
                .or(base.collapse_simple_statement),
            line_endings: self.line_endings.or(base.line_endings),
            block_newline_gaps: self.block_newline_gaps.or(base.block_newline_gaps),
            sort_requires: self.sort_requires.or(base.sort_requires),
            space_after_function_names: self
                .space_after_function_names
                .or(base.space_after_function_names),
            magic_trailing_comma: self.magic_trailing_comma.or(base.magic_trailing_comma),
        }
    }
}

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
