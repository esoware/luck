//! The enums live here so [`FormatConfig`] can deserialize straight into
//! them. `luck_formatter` re-exports every one, and its `FormatOptions`
//! holds the default each one takes when no config sets it.

/// Formatter settings from luck.json.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FormatConfig {
    pub line_width: Option<u16>,
    pub indent_style: Option<IndentStyle>,
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
    /// or `local NAME = game:GetService(...)` statements by name. A blank
    /// line ends a run.
    pub sort_requires: Option<bool>,
    /// Where to put a space before `(`: `"never"` (default), `"definitions"`,
    /// `"calls"`, or `"always"` for both.
    pub space_after_function_names: Option<SpaceAfterFunction>,
    /// When true, a trailing comma in a table constructor breaks the table one
    /// field per line, as in Black and Prettier. Defaults to `false`. Argument
    /// lists can't carry one, since no dialect allows a comma after the last
    /// argument.
    pub magic_trailing_comma: Option<bool>,
}

impl FormatConfig {
    #[must_use]
    pub(super) fn merge_onto(self, base: Self) -> Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IndentStyle {
    Tabs,
    Spaces,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
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
/// prefix (`0x`, `0b`) and exponent markers (`e`, `p`) are always lowercase,
/// so this setting only governs the digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HexCase {
    /// Leave the author's digit case untouched (`0xB0` and `0xb0` both stand).
    Preserve,
    /// `0xB0` -> `0xb0`.
    Lower,
    /// `0xb0` -> `0xB0`.
    Upper,
}

/// When a call drops the parentheses around a lone string or table argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineEndings {
    Unix,
    Windows,
}

/// Which simple single-statement blocks collapse onto one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CollapseSimpleStatement {
    /// Always expand blocks.
    Never,
    /// Simple function bodies, as in `function() return x end`. The formatter
    /// doesn't collapse function bodies, so this behaves like `never`.
    FunctionOnly,
    /// Collapse simple conditionals: `if x then return y end`.
    ConditionalOnly,
    /// Simple functions and conditionals. The formatter doesn't collapse
    /// function bodies, so this behaves like `conditional_only`.
    Always,
}

/// Whether a block body can start or end with a blank line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockNewlineGaps {
    /// Strip blank lines at the start and end of every block body.
    Never,
    /// Keep one blank line at the start or end of a block body when the source
    /// has any there.
    Preserve,
}

/// Where a space goes before the `(` of a function definition or call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_luck_config;

    #[test]
    fn parses_every_format_option() {
        let json = r#"{
            "format": {
                "line_width": 120,
                "indent_style": "spaces",
                "indent_width": 4,
                "quote_style": "double",
                "hexadecimal_case": "upper",
                "call_parentheses": "input",
                "collapse_simple_statement": "conditional_only",
                "line_endings": "windows",
                "block_newline_gaps": "preserve",
                "sort_requires": true,
                "space_after_function_names": "calls",
                "magic_trailing_comma": true
            }
        }"#;
        let config = parse_luck_config(json).expect("deserialize failed");
        let format = config.format.expect("format should be present");
        assert_eq!(format.line_width, Some(120));
        assert_eq!(format.indent_style, Some(IndentStyle::Spaces));
        assert_eq!(format.indent_width, Some(4));
        assert_eq!(format.quote_style, Some(QuoteStyle::Double));
        assert_eq!(format.hexadecimal_case, Some(HexCase::Upper));
        assert_eq!(format.call_parentheses, Some(CallParentheses::Input));
        assert_eq!(
            format.collapse_simple_statement,
            Some(CollapseSimpleStatement::ConditionalOnly)
        );
        assert_eq!(format.line_endings, Some(LineEndings::Windows));
        assert_eq!(format.block_newline_gaps, Some(BlockNewlineGaps::Preserve));
        assert_eq!(format.sort_requires, Some(true));
        assert_eq!(format.space_after_function_names, Some(SpaceAfterFunction::Calls));
        assert_eq!(format.magic_trailing_comma, Some(true));
    }

    #[test]
    fn leaves_unset_format_options_none() {
        let config = parse_luck_config(r#"{ "format": {} }"#).expect("deserialize failed");
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
        } = config.format.expect("format should be present");
        assert!(line_width.is_none());
        assert!(indent_style.is_none());
        assert!(indent_width.is_none());
        assert!(quote_style.is_none());
        assert!(hexadecimal_case.is_none());
        assert!(call_parentheses.is_none());
        assert!(collapse_simple_statement.is_none());
        assert!(line_endings.is_none());
        assert!(block_newline_gaps.is_none());
        assert!(sort_requires.is_none());
        assert!(space_after_function_names.is_none());
        assert!(magic_trailing_comma.is_none());
    }

    #[test]
    fn rejects_an_unknown_format_key() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "format": {
                "line_width": 100,
                "not_a_real_option": true
            }
        }"#;
        let result = parse_luck_config(json);
        assert!(result.is_err(), "unknown nested keys must error");
        let error = result.unwrap_err();
        assert!(
            error.contains("not_a_real_option") || error.contains("unknown field"),
            "error should name the offending nested key: {error}"
        );
    }

    #[test]
    fn rejects_an_unknown_quote_style() {
        let json = r#"{
            "lua": "lua54",
            "entry": "main.lua",
            "output_dir": "dist",
            "format": {
                "quote_style": "bogus"
            }
        }"#;
        let result = parse_luck_config(json);
        assert!(result.is_err(), "invalid enum values must error");
        let error = result.unwrap_err();
        assert!(
            error.contains("bogus") || error.contains("unknown variant"),
            "error should name the offending value: {error}"
        );
    }
}
