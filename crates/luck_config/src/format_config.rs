//! The `format` section. Each field is the `luck_formatter` option of the
//! same name, unset when the config leaves it out, and `FormatOptions` holds
//! the default it takes then.

use luck_formatter::{
    BlockNewlineGaps, CallParentheses, CollapseSimpleStatement, FormatOptions, HexCase,
    IndentStyle, LineEndings, QuoteStyle, SpaceAfterFunction,
};

/// Formatter settings from luck.json.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[cfg_attr(test, derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub(crate) struct FormatConfig {
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
    /// `true`. Argument lists can't carry one, since no dialect allows a comma
    /// after the last argument.
    pub magic_trailing_comma: Option<bool>,
}

impl FormatConfig {
    pub(crate) fn merge_onto(self, base: Self) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::parsed;
    use crate::parse::parse;

    /// The message `json` fails to parse with.
    fn parse_error(json: &str) -> String {
        parse(json).expect_err("config doesn't parse").message.to_string()
    }

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
        let format = parsed(json).format.expect("format should be present");
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
        let config = parsed(r#"{ "format": {} }"#);
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
        let error =
            parse_error(r#"{ "format": { "line_width": 100, "not_a_real_option": true } }"#);
        assert!(error.contains("unknown field `not_a_real_option`"), "{error}");
    }

    #[test]
    fn rejects_an_unknown_quote_style() {
        let error = parse_error(r#"{ "format": { "quote_style": "bogus" } }"#);
        assert!(error.contains("unknown variant `bogus`"), "{error}");
    }

    #[test]
    fn rejects_the_function_collapse_modes() {
        for mode in ["function_only", "always"] {
            let json = format!(r#"{{ "format": {{ "collapse_simple_statement": "{mode}" }} }}"#);
            let error = parse_error(&json);
            assert!(error.contains(mode), "error should name the offending value: {error}");
        }
    }
}
