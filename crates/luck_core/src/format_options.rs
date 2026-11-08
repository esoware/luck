//! Formatter option enums.
//!
//! They live here so [`FormatConfig`](crate::FormatConfig) can
//! deserialize straight into them. `luck_formatter` re-exports every one.

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HexCase {
    /// Leave the author's digit case untouched (`0xB0` and `0xb0` both stand).
    #[default]
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineEndings {
    #[default]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockNewlineGaps {
    /// Strip blank lines at the start and end of every block body.
    #[default]
    Never,
    /// Keep one blank line at the start or end of a block body when the source
    /// has any there.
    Preserve,
}

/// Where a space goes before the `(` of a function definition or call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpaceAfterFunction {
    /// No space: `function foo()`, `foo()`.
    #[default]
    Never,
    /// Space before a definition's parameter list only: `function foo ()`.
    Definitions,
    /// Space between a callee and its `(` only: `foo ()`.
    Calls,
    /// Space in both definitions and calls.
    Always,
}
