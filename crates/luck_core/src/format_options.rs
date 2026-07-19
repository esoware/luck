//! Formatter option enums shared between `luck_formatter` and config parsing.
//!
//! These live here (rather than in `luck_formatter`) so that
//! [`config::FormatConfig`](crate::config) can deserialize directly into them.
//! `luck_formatter` re-exports every type so existing `luck_formatter::Xxx`
//! paths keep resolving.

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
/// prefix (`0x`, `0b`) and exponent markers (`e`, `p`) are always lowered
/// regardless - uppercase `0X` has no stylistic constituency - so this only
/// governs the digits.
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

/// Controls when parentheses are used around single-argument function calls.
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
    /// Preserve the source's choice: `f"x"` stays bare, `f("x")` stays parenthesized.
    Input,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineEndings {
    #[default]
    Unix,
    Windows,
}

/// Controls collapsing of simple single-statement blocks onto one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CollapseSimpleStatement {
    /// Never collapse - always expand blocks.
    Never,
    /// Collapse simple function bodies: `function() return x end`.
    FunctionOnly,
    /// Collapse simple conditionals: `if x then return y end`.
    ConditionalOnly,
    /// Collapse both simple functions and conditionals.
    Always,
}

/// Controls whether blank lines inside blocks are preserved from source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockNewlineGaps {
    /// Strip blank lines at the start/end of any block body.
    #[default]
    Never,
    /// Keep blank lines at the start/end of block bodies verbatim.
    Preserve,
}

/// Controls inserting a space between `function`/callee and `(`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpaceAfterFunction {
    /// No space inserted: `function foo()` / `foo()`.
    #[default]
    Never,
    /// Space only after `function` keyword in definitions: `function foo ()`.
    Definitions,
    /// Space only between callee and `(` in calls: `foo ()`.
    Calls,
    /// Space in both definitions and calls.
    Always,
}
