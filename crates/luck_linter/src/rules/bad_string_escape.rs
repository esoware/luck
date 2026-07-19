use luck_token::{Comment, LuaVersion, Span};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Scan short-string literals in the raw source for backslash escapes
/// that are not recognized by the target Lua version. The lexer also
/// rejects these, but does so by aborting the string and discarding the
/// AST node - so a downstream rule that only looked at the AST would
/// never see the offending literal. Walking the source directly lets
/// this rule produce a single focused diagnostic per bad escape with
/// a help message tied to the target version's accepted set, which is
/// strictly more actionable than the lexer's "invalid escape sequence"
/// message and survives parse recovery on later statements.
pub struct BadStringEscape;

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

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let _block = ctx.block;
        let semantic = ctx.semantic;
        let source = ctx.source;
        let comments = ctx.comments;
        let mut diagnostics = Vec::new();
        scan_source(source, semantic.version, comments, &mut diagnostics);
        diagnostics
    }
}

/// Whether the byte after `\` is recognized for this Lua version. The
/// returned length is the count of source bytes after the backslash
/// the escape consumes; the caller uses it to advance the scanner.
fn escape_length(rest: &[u8], version: LuaVersion) -> Option<usize> {
    let first = *rest.first()?;
    match first {
        b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'"' | b'\'' | b'\n' => Some(1),
        b'\r' => {
            // CRLF or bare CR - both treated as one logical newline.
            if rest.get(1) == Some(&b'\n') {
                Some(2)
            } else {
                Some(1)
            }
        }
        b'x' if version.has_hex_escape() => {
            // `\xHH` - two hex digits required.
            if rest.len() >= 3 && is_hex(rest[1]) && is_hex(rest[2]) {
                Some(3)
            } else {
                None
            }
        }
        b'z' if version.has_whitespace_escape() => Some(1),
        b'u' if version.has_unicode_escape() => {
            // `\u{HHHH}` - at least one hex digit between braces.
            if rest.get(1) != Some(&b'{') {
                return None;
            }
            let mut idx = 2usize;
            let mut digits = 0usize;
            while let Some(&byte) = rest.get(idx) {
                if is_hex(byte) {
                    digits += 1;
                    idx += 1;
                } else {
                    break;
                }
            }
            if digits == 0 || rest.get(idx) != Some(&b'}') {
                return None;
            }
            Some(idx + 1)
        }
        b'0'..=b'9' => {
            // `\ddd` - up to three decimal digits.
            let mut idx = 1usize;
            while idx < 3 && matches!(rest.get(idx), Some(byte) if byte.is_ascii_digit()) {
                idx += 1;
            }
            Some(idx)
        }
        _ => None,
    }
}

fn is_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

/// Iterate raw source byte-by-byte, skipping comment regions, then
/// scan inside short-string literals for invalid escapes. Long-bracket
/// strings (`[[...]]`, `[=[...]=]`) do not process escapes at all and
/// are skipped wholesale.
fn scan_source(
    source: &str,
    version: LuaVersion,
    comments: &[Comment],
    diagnostics: &mut Vec<LintDiagnostic>,
) {
    let bytes = source.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        // Skip any comment whose span covers this byte. The comment list
        // is sorted, so a linear scan is fine for the realistic counts.
        if let Some(end) = comment_end_covering(comments, idx as u32) {
            idx = end as usize;
            continue;
        }
        let byte = bytes[idx];
        if byte == b'"' || byte == b'\'' {
            idx = scan_short_string(bytes, idx, version, diagnostics);
            continue;
        }
        if byte == b'[' {
            if let Some(skip_to) = skip_long_bracket(bytes, idx) {
                idx = skip_to;
                continue;
            }
        }
        idx += 1;
    }
}

fn comment_end_covering(comments: &[Comment], byte_offset: u32) -> Option<u32> {
    for comment in comments {
        if comment.span.start <= byte_offset && byte_offset < comment.span.end {
            return Some(comment.span.end);
        }
        if comment.span.start > byte_offset {
            return None;
        }
    }
    None
}

/// Skip a Lua long-bracket string `[==[...]==]`. Returns the offset
/// after the closing bracket if the opener parses, otherwise None.
fn skip_long_bracket(bytes: &[u8], start: usize) -> Option<usize> {
    let mut idx = start + 1;
    let mut level = 0usize;
    while bytes.get(idx) == Some(&b'=') {
        level += 1;
        idx += 1;
    }
    if bytes.get(idx) != Some(&b'[') {
        return None;
    }
    idx += 1;
    while idx < bytes.len() {
        if bytes[idx] == b']' {
            let mut close = idx + 1;
            let mut close_level = 0usize;
            while bytes.get(close) == Some(&b'=') {
                close_level += 1;
                close += 1;
            }
            if close_level == level && bytes.get(close) == Some(&b']') {
                return Some(close + 1);
            }
        }
        idx += 1;
    }
    Some(bytes.len())
}

/// Scan one short-string literal starting at `start` (the quote byte).
/// Returns the offset after the closing quote (or end of file on an
/// unterminated string).
fn scan_short_string(
    bytes: &[u8],
    start: usize,
    version: LuaVersion,
    diagnostics: &mut Vec<LintDiagnostic>,
) -> usize {
    let quote = bytes[start];
    let mut idx = start + 1;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\\' => {
                let after = &bytes[idx + 1..];
                match escape_length(after, version) {
                    Some(consumed) => idx += 1 + consumed,
                    None => {
                        let bad_char = after.first().copied().unwrap_or(b'?');
                        let display = if bad_char.is_ascii_graphic() {
                            format!("\\{}", bad_char as char)
                        } else {
                            format!("\\x{bad_char:02x}")
                        };
                        let backslash = idx as u32;
                        diagnostics.push(LintDiagnostic::new("bad_string_escape", format!("invalid string escape `{display}`"), Span::new(backslash, backslash + 2)).with_help(
                                "valid escapes: \\a \\b \\f \\n \\r \\t \\v \\\\ \\\" \\' \\xHH \\ddd"
                                    .to_string(),
                            ));
                        idx += if after.is_empty() { 1 } else { 2 };
                    }
                }
            }
            b if b == quote => {
                return idx + 1;
            }
            b'\n' => {
                // Unterminated short string - bail out, the lexer will
                // raise its own diagnostic; we don't want to chase the
                // scanner into the next statement.
                return idx;
            }
            _ => idx += 1,
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        // We deliberately do not assert parse.errors.is_empty(): the
        // lexer also rejects these escapes, but the rule operates on
        // raw source and is responsible for surfacing its own diagnostic.
        let parse = luck_parser::parse(source, version);
        let semantic = luck_semantic::analyze(&parse.block, version);
        BadStringEscape.check(&crate::rule::LintContext {
            block: &parse.block,
            semantic: &semantic,
            source,
            comments: &parse.comments,
            config: &crate::LintConfig::default(),
        })
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
        let diags = run(r#"local x = [[\q]]"#, LuaVersion::Lua54);
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
}
