use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags backslash escapes in strings that the dialect doesn't define, which
/// Lua 5.1 and Luau read as the bare character.
///
/// The lexer notes each where it reads it. Every other version rejects these
/// in the lexer, so a file on those targets that reaches the rules has none
/// to flag.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BadStringEscape;

declare_lint_rule!(
    BadStringEscape,
    Correctness,
    Warning,
    "invalid backslash escape in string literal"
);

impl Rule for BadStringEscape {
    fn run_once(&self, context: &LintContext<'_>) {
        if context.lenient_escapes.is_empty() {
            return;
        }
        let extended_escapes = if context.semantic.version().has_hex_escape() {
            " `\\xXX` `\\z` `\\u{XXX}`"
        } else {
            ""
        };
        let help = format!(
            "valid escapes: `\\a` `\\b` `\\f` `\\n` `\\r` `\\t` `\\v` `\\\\` `\\\"` `\\'` \
             `\\ddd`{extended_escapes}"
        );
        for &span in context.lenient_escapes {
            let escaped = span.text(context.source).get(1..).unwrap_or_default();
            // The escape takes one byte, and its span the whole character.
            let display = match str::from_utf8(escaped).ok().and_then(|text| text.chars().next()) {
                Some(character) if character.is_ascii_control() || character == ' ' => {
                    format!("\\x{:02x}", u32::from(character))
                }
                Some(character) => format!("\\{character}"),
                None => format!("\\x{:02x}", escaped.first().copied().unwrap_or_default()),
            };
            context.diagnostic(
                Finding::new(format!("invalid string escape `{display}`"), span)
                    .with_help(help.clone()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{LuaVersion, offset_to_index};

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str, version: LuaVersion) -> Vec<Message> {
        run_rule(RuleId::BadStringEscape, source, version)
    }

    #[test]
    fn flags_unknown_letter_escape() {
        let diags = run(r#"local x = "\q""#, LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("\\q"));
    }

    #[test]
    fn ignores_newline_escape() {
        let diags = run(r#"local x = "\n""#, LuaVersion::Lua51);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_z_on_lua51() {
        let diags = run(r#"local x = "\z""#, LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_z_on_lua52() {
        let diags = run(r#"local x = "\z""#, LuaVersion::Lua52);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_unicode_escape_on_lua51() {
        let diags = run(r#"local x = "\u{1F600}""#, LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_unicode_escape_on_lua53() {
        let diags = run(r#"local x = "\u{1F600}""#, LuaVersion::Lua53);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_long_bracket_escape() {
        let diags = run(r"local x = [[\q]]", LuaVersion::Lua51);
        assert!(diags.is_empty(), "long brackets ignored: {diags:?}");
    }

    #[test]
    fn flags_hex_escape_on_lua51() {
        let diags = run(r#"local x = "\x41""#, LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_decimal_escape() {
        let diags = run(r#"local x = "\065""#, LuaVersion::Lua51);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_escape_inside_line_comment() {
        let diags = run(
            "\
-- a \\q comment
local x = 1
",
            LuaVersion::Lua51,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_escapes_between_comments() {
        let source = "\
-- \"\\q
local x = \"\\q\" --[[ '\\q ]] local y = '\\w' -- \\e
";
        let diags = run(source, LuaVersion::Lua51);
        let flagged: Vec<&[u8]> =
            diags.iter().map(|diag| diag.span().text(source.as_bytes())).collect();
        assert_eq!(flagged, [b"\\q", b"\\w"], "{diags:?}");
        assert_eq!(offset_to_index(diags[0].span().start), source.find("= \"").unwrap() + 3);
    }

    #[test]
    fn ignores_brace_escape_after_a_quote_in_a_backtick_string() {
        for source in ["local a = `it's \\{x}`", "local b = `say \"\\{hi}\" {x}`"] {
            let diags = run(source, LuaVersion::Luau);
            assert!(diags.is_empty(), "{source}: {diags:?}");
        }
    }

    #[test]
    fn flags_unknown_letter_escape_on_luau() {
        let diags = run(r#"local x = "\q""#, LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_luau_hex_whitespace_and_unicode_escapes() {
        let diags = run(r#"local x = "\x41\z  \u{41}""#, LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_unknown_escapes_in_interpolated_string_segments() {
        let source = "local x = `a\\q{1}b\\w`";
        let diags = run(source, LuaVersion::Luau);
        let flagged: Vec<&[u8]> =
            diags.iter().map(|diag| diag.span().text(source.as_bytes())).collect();
        assert_eq!(flagged, [b"\\q", b"\\w"], "{diags:?}");
    }

    #[test]
    fn flags_unknown_escapes_in_a_backtick_string_without_holes() {
        let source = "local x = `a\\qb\\{c}\\w`";
        let diags = run(source, LuaVersion::Luau);
        let flagged: Vec<&[u8]> =
            diags.iter().map(|diag| diag.span().text(source.as_bytes())).collect();
        assert_eq!(flagged, [b"\\q", b"\\w"], "{diags:?}");
    }

    #[test]
    fn ignores_valid_escapes_in_a_backtick_string_without_holes() {
        let diags = run(r"local x = `\{\}\`\n\x41\z  \u{41}\065`", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_unknown_escape_in_a_string_call_argument() {
        let diags = run(r#"print "\q""#, LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn span_covers_whole_escaped_character() {
        let source = "local x = \"\\\u{e9}\"";
        let diags = run(source, LuaVersion::Lua51);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags[0].span().text(source.as_bytes()),
            "\\\u{e9}".as_bytes(),
            "the span should cover the whole character: {diags:?}"
        );
    }
}
