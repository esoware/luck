use luck_token::{Comment, LuaVersion, Span, index_to_offset, offset_to_index};

use crate::diagnostic::{Category, Finding, Severity};
use crate::rule::{LintContext, Rule, WholeRule};

/// Flags backslash escapes in short strings that the target version
/// doesn't define.
///
/// It reads the raw source because in Lua 5.2+ and Luau the lexer rejects
/// these escapes and drops the string, so the AST never holds it. Lua 5.1
/// reads an unknown escape as the bare character, and it's flagged there
/// too.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BadStringEscape;

impl Rule for BadStringEscape {
    fn name(&self) -> &'static str {
        "bad_string_escape"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "invalid backslash escape in string literal"
    }
}

impl WholeRule for BadStringEscape {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        scan_source(context.source, context.semantic.version, context.comments)
    }
}

/// Returns how many bytes after the `\` an escape spans, or `None` if
/// `version` doesn't define it.
fn escape_length(rest: &[u8], version: LuaVersion) -> Option<usize> {
    match rest {
        [b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'"' | b'\'' | b'\n', ..] => {
            Some(1)
        }
        // CRLF and bare CR are both one logical newline.
        [b'\r', after_cr @ ..] => Some(if after_cr.first() == Some(&b'\n') { 2 } else { 1 }),
        // Lua 5.2+ and Luau: `\xHH` requires two hex digits.
        [b'x', high, low, ..]
            if version.has_hex_escape() && high.is_ascii_hexdigit() && low.is_ascii_hexdigit() =>
        {
            Some(3)
        }
        // Lua 5.2+ and Luau.
        [b'z', ..] if version.has_whitespace_escape() => Some(1),
        // Lua 5.3+ and Luau: `\u{HHHH}` needs at least one hex digit between
        // braces.
        [b'u', b'{', digits @ ..] if version.has_unicode_escape() => {
            let digit_count = digits.iter().take_while(|byte| byte.is_ascii_hexdigit()).count();
            (digit_count > 0 && digits.get(digit_count) == Some(&b'}'))
                .then(|| digit_count.saturating_add(3))
        }
        // `\ddd` takes up to three decimal digits.
        [b'0'..=b'9', after_first @ ..] => {
            let extra_digit_count =
                after_first.iter().take(2).take_while(|byte| byte.is_ascii_digit()).count();
            Some(extra_digit_count.saturating_add(1))
        }
        _ => None,
    }
}

/// Walks the source outside comments and checks each short string's
/// escapes.
///
/// Long-bracket strings (`[[...]]`, `[=[...]=]`) have no escapes, so
/// they're skipped whole.
fn scan_source(source: &str, version: LuaVersion, comments: &[Comment]) -> Vec<Finding> {
    let bytes = source.as_bytes();
    let mut findings = Vec::new();
    // The list is sorted and the scan only moves forward, so a comment that
    // ends behind the scan is done with.
    let mut pending_comments = comments.iter().peekable();
    let mut index = 0usize;
    while let Some(&byte) = bytes.get(index) {
        let offset = index_to_offset(index);
        while pending_comments.next_if(|comment| comment.span.end <= offset).is_some() {}
        if let Some(comment) = pending_comments.peek()
            && comment.span.start <= offset
        {
            index = offset_to_index(comment.span.end);
            continue;
        }
        index = if byte == b'"' || byte == b'\'' {
            scan_short_string(bytes, index, byte, version, &mut findings)
        } else if byte == b'['
            && let Some(skip_to) = skip_long_bracket(bytes, index)
        {
            skip_to
        } else {
            index.saturating_add(1)
        };
    }
    findings
}

/// Returns the offset just past a long-bracket string `[==[...]==]` that
/// opens at `start`, or `None` if no opener is there.
///
/// An unterminated string runs to the end of the source.
fn skip_long_bracket(bytes: &[u8], start: usize) -> Option<usize> {
    let after_bracket = bytes.get(start.saturating_add(1)..)?;
    let level = after_bracket.iter().take_while(|&&byte| byte == b'=').count();
    let [b'[', body @ ..] = after_bracket.get(level..)? else {
        return None;
    };
    // The opener and closer are the same width: two brackets around
    // `level` equals signs.
    let delimiter_length = level.saturating_add(2);
    let body_start = start.saturating_add(delimiter_length);
    let closer = body.windows(delimiter_length).position(|window| {
        matches!(window, [b']', equals @ .., b']'] if equals.iter().all(|&byte| byte == b'='))
    });
    Some(closer.map_or(bytes.len(), |closer_index| {
        body_start.saturating_add(closer_index).saturating_add(delimiter_length)
    }))
}

/// Checks the escapes in the short string whose opening `quote` is at
/// `start`, and returns the offset after its closing quote.
///
/// An unterminated string stops at its newline or the end of the source.
fn scan_short_string(
    bytes: &[u8],
    start: usize,
    quote: u8,
    version: LuaVersion,
    findings: &mut Vec<Finding>,
) -> usize {
    let mut index = start.saturating_add(1);
    while let Some(&byte) = bytes.get(index) {
        let after_byte = index.saturating_add(1);
        if byte == quote {
            return after_byte;
        }
        match byte {
            b'\\' => {
                let after = bytes.get(after_byte..).unwrap_or_default();
                if let Some(consumed) = escape_length(after, version) {
                    index = after_byte.saturating_add(consumed);
                } else {
                    let bad_char = after.first().copied().unwrap_or(b'?');
                    let display = if bad_char.is_ascii_graphic() {
                        format!("\\{}", char::from(bad_char))
                    } else {
                        format!("\\x{bad_char:02x}")
                    };
                    findings.push(
                        Finding::new(
                            format!("invalid string escape `{display}`"),
                            Span::from_indices(index, index.saturating_add(2)),
                        )
                        .with_help(
                            "valid escapes: `\\a` `\\b` `\\f` `\\n` `\\r` `\\t` \
                             `\\v` `\\\\` `\\\"` `\\'` `\\ddd`, plus `\\xHH` and `\\z` \
                             (Lua 5.2+, Luau) and `\\u{XXX}` (Lua 5.3+, Luau)",
                        ),
                    );
                    index = after_byte.saturating_add(after.len().min(1));
                }
            }
            b'\n' => {
                // Unterminated. The lexer reports it, and scanning on into
                // the next line would only add noise.
                return index;
            }
            _ => index = after_byte,
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule_on_parse;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        // The lexer rejects these escapes too, so the parse may have errors.
        run_rule_on_parse(&BadStringEscape, &luck_parser::parse(source, version), version)
    }

    #[test]
    fn flags_unknown_letter_escape() {
        let diags = run(r#"local x = "\q""#, LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("\\q"));
    }

    #[test]
    fn ignores_newline_escape() {
        let diags = run(r#"local x = "\n""#, LuaVersion::Lua54);
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
    fn flags_unicode_escape_on_lua52() {
        let diags = run(r#"local x = "\u{1F600}""#, LuaVersion::Lua52);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_unicode_escape_on_lua53() {
        let diags = run(r#"local x = "\u{1F600}""#, LuaVersion::Lua53);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_long_bracket_escape() {
        let diags = run(r"local x = [[\q]]", LuaVersion::Lua54);
        assert!(diags.is_empty(), "long brackets ignored: {diags:?}");
    }

    #[test]
    fn flags_bad_hex_escape() {
        let diags = run(r#"local x = "\xZZ""#, LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_decimal_escape() {
        let diags = run(r#"local x = "\065""#, LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_escape_inside_line_comment() {
        let diags = run("-- a \\q comment\nlocal x = 1\n", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_escapes_between_comments() {
        let source = "-- \"\\q\nlocal x = \"\\q\" --[[ '\\q ]] local y = '\\w' -- \\e\n";
        let diags = run(source, LuaVersion::Lua54);
        let flagged: Vec<&str> = diags.iter().map(|diag| diag.span.text(source)).collect();
        assert_eq!(flagged, ["\\q", "\\w"], "{diags:?}");
        assert_eq!(offset_to_index(diags[0].span.start), source.find("= \"").unwrap() + 3);
    }
}
