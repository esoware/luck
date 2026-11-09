use luck_ast::{Expression, FunctionArgs, FunctionCall, Literal};
use luck_token::{LuaVersion, Span, Token, TokenKind, index_to_offset, short_string_body};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

/// Flags backslash escapes in short strings that the dialect doesn't define,
/// which Lua 5.1 and Luau read as the bare character.
///
/// Every other version rejects these in the lexer, so a file on those targets
/// that reaches the rules has none left to flag.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BadStringEscape;

impl_rule!(
    BadStringEscape,
    "bad_string_escape",
    Correctness,
    Warning,
    "invalid backslash escape in string literal"
);

impl NodeRule for BadStringEscape {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[
            NodeType::StringLiteral,
            NodeType::InterpolatedString,
            NodeType::FunctionCallStmt,
            NodeType::FunctionCallExpr,
        ])
    }

    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let version = context.semantic.version;
        if let Expression::StringLiteral(literal) = expr {
            check_literal(literal, version, findings);
        }
        if let Expression::InterpolatedString(interpolated) = expr {
            for segment in &interpolated.segments {
                check_segment(&segment.literal, version, findings);
            }
        }
    }

    fn on_call(
        &self,
        call: &FunctionCall,
        _position: CallPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let FunctionArgs::StringLiteral(literal) = &call.args {
            check_literal(literal, context.semantic.version, findings);
        }
    }
}

/// Long-bracket strings have no escapes, so only a quoted body is checked.
fn check_literal(literal: &Literal, version: LuaVersion, findings: &mut Vec<Finding>) {
    if let Some(body) = short_string_body(&literal.text) {
        check_escapes(body, literal.span.start.saturating_add(1), false, version, findings);
    }
}

fn check_segment(literal: &Token, version: LuaVersion, findings: &mut Vec<Finding>) {
    let (TokenKind::InterpolatedStringBegin(text)
    | TokenKind::InterpolatedStringMiddle(text)
    | TokenKind::InterpolatedStringEnd(text)) = &literal.kind
    else {
        return;
    };
    // Every segment's span ends with its one-byte delimiter.
    let body_start = literal.span.end.saturating_sub(1).saturating_sub(index_to_offset(text.len()));
    check_escapes(text, body_start, true, version, findings);
}

fn escape_length(rest: &[u8], is_interpolated: bool, version: LuaVersion) -> Option<usize> {
    match rest {
        [b'\r', b'\n', ..] => Some(2),
        [b'\n', b'\r', ..] if version.has_lfcr_line_break() => Some(2),
        [
            b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'"' | b'\'' | b'\n' | b'\r',
            ..,
        ] => Some(1),
        [b'x', ..] if version.has_hex_escape() => Some(1),
        [b'z', ..] if version.has_whitespace_escape() => Some(1),
        [b'u', ..] if version.has_unicode_escape() => Some(1),
        // Luau: an escaped delimiter is literal text.
        [b'{' | b'}' | b'`', ..] if is_interpolated => Some(1),
        // `\ddd` takes up to three decimal digits.
        [b'0'..=b'9', after_first @ ..] => {
            let extra_digit_count =
                after_first.iter().take(2).take_while(|b| b.is_ascii_digit()).count();
            Some(extra_digit_count.saturating_add(1))
        }
        _ => None,
    }
}

fn check_escapes(
    body: &str,
    body_start: u32,
    is_interpolated: bool,
    version: LuaVersion,
    findings: &mut Vec<Finding>,
) {
    if version.has_strict_escapes() {
        return;
    }
    let bytes = body.as_bytes();
    let extended_escapes = if version.has_hex_escape() { " `\\xXX` `\\z` `\\u{XXX}`" } else { "" };
    let mut index = 0usize;
    while let Some(&byte) = bytes.get(index) {
        let after_byte = index.saturating_add(1);
        if byte != b'\\' {
            index = after_byte;
            continue;
        }
        let after = bytes.get(after_byte..).unwrap_or_default();
        if let Some(consumed) = escape_length(after, is_interpolated, version) {
            index = after_byte.saturating_add(consumed);
        } else {
            let bad_char =
                body.get(after_byte..).and_then(|rest| rest.chars().next()).unwrap_or('?');
            let display = if bad_char.is_ascii_control() || bad_char == ' ' {
                format!("\\x{:02x}", u32::from(bad_char))
            } else {
                format!("\\{bad_char}")
            };
            let escape_start = body_start.saturating_add(index_to_offset(index));
            let escape_width = bad_char.len_utf8().saturating_add(1);
            findings.push(
                Finding::new(
                    format!("invalid string escape `{display}`"),
                    Span::new(
                        escape_start,
                        escape_start.saturating_add(index_to_offset(escape_width)),
                    ),
                )
                .with_help(format!(
                    "valid escapes: `\\a` `\\b` `\\f` `\\n` `\\r` `\\t` `\\v` `\\\\` `\\\"` `\\'` \
                     `\\ddd`{extended_escapes}"
                )),
            );
            index = after_byte.saturating_add(bad_char.len_utf8());
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::offset_to_index;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        run_rule(&BadStringEscape, source, version)
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
        let diags = run("-- a \\q comment\nlocal x = 1\n", LuaVersion::Lua51);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_escapes_between_comments() {
        let source = "-- \"\\q\nlocal x = \"\\q\" --[[ '\\q ]] local y = '\\w' -- \\e\n";
        let diags = run(source, LuaVersion::Lua51);
        let flagged: Vec<&str> = diags.iter().map(|diag| diag.span.text(source)).collect();
        assert_eq!(flagged, ["\\q", "\\w"], "{diags:?}");
        assert_eq!(offset_to_index(diags[0].span.start), source.find("= \"").unwrap() + 3);
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
        let flagged: Vec<&str> = diags.iter().map(|diag| diag.span.text(source)).collect();
        assert_eq!(flagged, ["\\q", "\\w"], "{diags:?}");
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
            diags[0].span.text(source),
            "\\\u{e9}",
            "the span should cover the whole character: {diags:?}"
        );
    }
}
