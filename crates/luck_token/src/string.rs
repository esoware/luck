//! Lua string literal semantics shared by every crate that folds, compares,
//! or re-emits string values.
//!
//! Working on raw token text instead of decoded values miscompiles: UTF-8
//! corruption, escape-state confusion, and `"\65"` comparing unequal to `"A"`.

use alloc::borrow::Cow;
use core::fmt::Write as _;

use compact_str::CompactString;

use crate::LuaVersion;

/// The text between a quoted string literal's quotes, escapes left undecoded,
/// so `"\65"` and `"A"` give different bodies.
///
/// Returns `None` for a long-bracket string and for any text that isn't a
/// quoted literal.
#[must_use]
pub fn short_string_body(raw: &str) -> Option<&str> {
    let strip_quote = |quote: char| raw.strip_prefix(quote)?.strip_suffix(quote);
    strip_quote('"').or_else(|| strip_quote('\''))
}

/// Decode a string literal token's raw text to its runtime bytes.
///
/// Handles both quote forms with every Lua escape, and long-bracket strings
/// (EOL normalization, leading-newline strip, no escapes). Returns `None` for
/// any form this doesn't model exactly, and a caller that gets `None` must
/// not fold. Lua 5.1 reads an undefined escape as the literal character, and
/// Luau's long strings keep a lone CR as content. The result borrows from
/// `raw` when the body needs no rewriting: a quoted string without escapes,
/// or a long-bracket string without a CR.
#[must_use]
pub fn decode_string(raw: &str, version: LuaVersion) -> Option<Cow<'_, [u8]>> {
    if raw.starts_with('[') {
        return decode_long_bracket(raw.as_bytes(), version);
    }

    let (&quote, after_quote) = raw.as_bytes().split_first()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let (_, inner) = after_quote.split_last()?;
    if !inner.contains(&b'\\') {
        return Some(Cow::Borrowed(inner));
    }
    let mut output = Vec::with_capacity(inner.len());
    let mut remaining = inner;
    while let Some((&byte, rest)) = remaining.split_first() {
        remaining = if byte == b'\\' {
            decode_escape(rest, version, &mut output)?
        } else {
            output.push(byte);
            rest
        };
    }
    Some(Cow::Owned(output))
}

fn decode_long_bracket(raw: &[u8], version: LuaVersion) -> Option<Cow<'_, [u8]>> {
    let after_open = raw.strip_prefix(b"[")?;
    let level = after_open.iter().take_while(|&&byte| byte == b'=').count();
    let delimiter_length = level.saturating_add(2);
    let content_end = raw.len().checked_sub(delimiter_length)?;
    let content = raw.get(delimiter_length..content_end).unwrap_or_default();
    // Each EOL sequence becomes one newline, and an EOL right after the
    // opening bracket is dropped (5.4 manual section 3.1). Every EOL but a
    // lone LF holds a CR, so a body without one is already the value.
    let body = strip_eol(content, version).unwrap_or(content);
    if !body.contains(&b'\r') {
        return Some(Cow::Borrowed(body));
    }
    let mut output = Vec::with_capacity(body.len());
    let mut remaining = body;
    while let Some((&byte, rest)) = remaining.split_first() {
        let (decoded, after_decoded) =
            strip_eol(remaining, version).map_or((byte, rest), |after_eol| (b'\n', after_eol));
        output.push(decoded);
        remaining = after_decoded;
    }
    Some(Cow::Owned(output))
}

/// The bytes after a leading EOL sequence, or `None` when there is none. PUC
/// recognizes CR, LF, CRLF, and LFCR; Luau only LF and CRLF, keeping a lone
/// CR as a literal byte (`Lexer::fixupMultilineString`).
const fn strip_eol(bytes: &[u8], version: LuaVersion) -> Option<&[u8]> {
    match bytes {
        [b'\r', b'\n', rest @ ..] => Some(rest),
        [b'\n', b'\r', rest @ ..] | [b'\r', rest @ ..] if !version.is_luau() => Some(rest),
        [b'\n', rest @ ..] => Some(rest),
        _ => None,
    }
}

/// Decode the escape in `escape`, which starts just past its `\`, and return
/// the bytes after it.
fn decode_escape<'text>(
    escape: &'text [u8],
    version: LuaVersion,
    output: &mut Vec<u8>,
) -> Option<&'text [u8]> {
    let (&escaped, rest) = escape.split_first()?;
    match escaped {
        b'a' => output.push(0x07),
        b'b' => output.push(0x08),
        b'f' => output.push(0x0C),
        b'n' => output.push(b'\n'),
        b'r' => output.push(b'\r'),
        b't' => output.push(b'\t'),
        b'v' => output.push(0x0B),
        b'\\' => output.push(b'\\'),
        b'"' => output.push(b'"'),
        b'\'' => output.push(b'\''),
        b'\n' => {
            output.push(b'\n');
            if !version.is_luau() {
                return Some(rest.strip_prefix(b"\r").unwrap_or(rest));
            }
        }
        b'\r' => {
            output.push(b'\n');
            return Some(rest.strip_prefix(b"\n").unwrap_or(rest));
        }
        b'x' if version.has_hex_escape() => return decode_hex_escape(rest, output),
        b'z' if version.has_whitespace_escape() => {
            let whitespace_length = rest
                .iter()
                .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C))
                .count();
            return rest.get(whitespace_length..);
        }
        b'0'..=b'9' => return decode_decimal_escape(escape, output),
        b'u' if version.has_unicode_escape() => return decode_unicode_escape(rest, output),
        // Lua 5.1 reads any other escaped character as itself. Strict
        // versions reject the token, so refuse to fold it.
        _ if !version.has_strict_escapes() => output.push(escaped),
        _ => return None,
    }
    Some(rest)
}

fn decode_hex_escape<'text>(rest: &'text [u8], output: &mut Vec<u8>) -> Option<&'text [u8]> {
    let [high_digit, low_digit, after_digits @ ..] = rest else {
        return None;
    };
    let high_nibble = char::from(*high_digit).to_digit(16)?;
    let low_nibble = char::from(*low_digit).to_digit(16)?;
    output.push(u8::try_from((high_nibble << 4u32) | low_nibble).ok()?);
    Some(after_digits)
}

fn decode_decimal_escape<'text>(escape: &'text [u8], output: &mut Vec<u8>) -> Option<&'text [u8]> {
    let digit_count = escape.iter().take(3).take_while(|byte| byte.is_ascii_digit()).count();
    let (digits, rest) = escape.split_at(digit_count);
    output.push(str::from_utf8(digits).ok()?.parse().ok()?);
    Some(rest)
}

fn decode_unicode_escape<'text>(rest: &'text [u8], output: &mut Vec<u8>) -> Option<&'text [u8]> {
    let mut remaining = rest.strip_prefix(b"{")?;
    let mut value: u32 = 0;
    while let Some((&digit, after_digit)) = remaining.split_first() {
        remaining = after_digit;
        if digit == b'}' {
            break;
        }
        value = value.checked_mul(16)?.checked_add(char::from(digit).to_digit(16)?)?;
    }
    let character = char::from_u32(value)?;
    let mut encoded = [0u8; 4];
    output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
    Some(remaining)
}

/// Encode runtime bytes back into a double-quoted literal.
///
/// Escapes double quotes, backslashes, and control bytes. Valid UTF-8 runs
/// pass through intact, and bytes that aren't valid UTF-8 (such as those from
/// `\xFF` escapes) become decimal escapes.
#[must_use]
pub fn encode_string(bytes: &[u8]) -> CompactString {
    let mut output = CompactString::with_capacity(bytes.len().saturating_add(2));
    output.push('"');
    if let Ok(text) = str::from_utf8(bytes) {
        push_escaped_text(&mut output, text);
    } else {
        let mut remaining = bytes;
        for chunk in bytes.utf8_chunks() {
            push_escaped_text(&mut output, chunk.valid());
            remaining = remaining.get(chunk.valid().len()..).unwrap_or_default();
            for &byte in chunk.invalid() {
                remaining = remaining.get(1..).unwrap_or_default();
                let is_followed_by_digit = remaining.first().is_some_and(u8::is_ascii_digit);
                push_decimal_escape(&mut output, byte, is_followed_by_digit);
            }
        }
    }
    output.push('"');
    output
}

/// Append valid UTF-8, copying the runs between bytes that need escaping.
///
/// Those bytes are all ASCII, so every split lands on a char boundary. `text`
/// ends at an invalid byte or the end of input, never a digit, so an escape
/// at its end needs no padding.
fn push_escaped_text(output: &mut CompactString, mut text: &str) {
    while let Some((index, byte)) =
        text.bytes().enumerate().find(|&(_, byte)| matches!(byte, 0..=0x1F | 0x7F | b'"' | b'\\'))
    {
        let (run, escaped) = text.split_at(index);
        output.push_str(run);
        text = escaped.get(1..).unwrap_or_default();
        match byte {
            b'"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            0x07 => output.push_str("\\a"),
            0x08 => output.push_str("\\b"),
            0x0C => output.push_str("\\f"),
            b'\t' => output.push_str("\\t"),
            0x0B => output.push_str("\\v"),
            _ => {
                let is_followed_by_digit = text.as_bytes().first().is_some_and(u8::is_ascii_digit);
                push_decimal_escape(output, byte, is_followed_by_digit);
            }
        }
    }
    output.push_str(text);
}

fn push_decimal_escape(output: &mut CompactString, byte: u8, is_followed_by_digit: bool) {
    // Pad to 3 digits when a digit follows so the escape can't
    // absorb it (`\9` + `9` must not read as `\99`).
    let written = if is_followed_by_digit {
        write!(output, "\\{byte:03}")
    } else {
        write!(output, "\\{byte}")
    };
    // Writing to a `CompactString` can't fail.
    written.unwrap_or_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(raw: &str) -> Option<Vec<u8>> {
        decode_in(raw, LuaVersion::Lua54)
    }

    fn decode_in(raw: &str, version: LuaVersion) -> Option<Vec<u8>> {
        decode_string(raw, version).map(Cow::into_owned)
    }

    #[test]
    fn short_string_body_keeps_escapes_raw() {
        assert_eq!(short_string_body(r#""a\65""#), Some(r"a\65"));
        assert_eq!(short_string_body("'b'"), Some("b"));
        assert_eq!(short_string_body("\"\""), Some(""));
    }

    #[test]
    fn short_string_body_rejects_unquoted_text() {
        for raw in ["[[a]]", "[==[a]==]", "\"", "'", "\"a'", "`a`", "12"] {
            assert_eq!(short_string_body(raw), None, "{raw}");
        }
    }

    #[test]
    fn decode_string_reads_an_escape_free_body_as_is() {
        assert_eq!(decode(r#""hello""#), Some(b"hello".to_vec()));
        assert_eq!(decode("'hi'"), Some(b"hi".to_vec()));
        assert_eq!(decode(r#""""#), Some(Vec::new()));
    }

    #[test]
    fn decode_string_borrows_only_bodies_that_need_no_rewriting() {
        for raw in [r#""hello""#, "'a\u{e9}b'", "[[a\nb]]", "[==[\nhi]==]", "[[]]"] {
            for version in LuaVersion::ALL {
                let decoded = decode_string(raw, version);
                assert!(matches!(decoded, Some(Cow::Borrowed(_))), "{raw:?} {version:?}");
            }
        }
        for raw in [r#""a\n""#, "[[a\r\nb]]", "[[\r\na\r\n]]"] {
            for version in LuaVersion::ALL {
                let decoded = decode_string(raw, version);
                assert!(matches!(decoded, Some(Cow::Owned(_))), "{raw:?} {version:?}");
            }
        }
    }

    #[test]
    fn decode_string_reads_named_escapes() {
        assert_eq!(decode(r#""\n""#), Some(vec![b'\n']));
        assert_eq!(decode(r#""\t""#), Some(vec![b'\t']));
        assert_eq!(decode(r#""\\""#), Some(vec![b'\\']));
        assert_eq!(decode(r#""\"""#), Some(vec![b'"']));
        assert_eq!(decode(r#""\a""#), Some(vec![0x07]));
        assert_eq!(decode(r#""\r""#), Some(vec![b'\r']));
    }

    #[test]
    fn decode_string_reads_hex_escapes() {
        assert_eq!(decode(r#""\x41""#), Some(vec![0x41]));
        assert_eq!(decode(r#""\xff""#), Some(vec![0xFF]));
    }

    #[test]
    fn decode_string_reads_decimal_escapes() {
        assert_eq!(decode(r#""\65""#), Some(vec![65]));
        assert_eq!(decode(r#""\9""#), Some(vec![9]));
        assert_eq!(decode(r#""\255""#), Some(vec![255]));
    }

    #[test]
    fn decode_string_refuses_decimal_escapes_past_255() {
        assert_eq!(decode(r#""\256""#), None);
        assert_eq!(decode(r#""\300""#), None);
    }

    #[test]
    fn decode_string_encodes_unicode_escapes_as_utf8() {
        assert_eq!(decode(r#""\u{48}""#), Some(b"H".to_vec()));
        // U+00E9 (e-acute) encodes to two UTF-8 bytes.
        assert_eq!(decode(r#""\u{e9}""#), Some(vec![0xC3, 0xA9]));
    }

    #[test]
    fn decode_string_skips_whitespace_after_z() {
        // `\z` skips the whitespace run that follows, including newlines.
        assert_eq!(decode("\"a\\z   \n   b\""), Some(b"ab".to_vec()));
    }

    #[test]
    fn decode_string_refuses_undefined_escapes_on_strict_versions() {
        assert_eq!(decode(r#""\q""#), None);
        assert_eq!(decode_in(r#""\q""#, LuaVersion::Luau), None);
    }

    #[test]
    fn decode_string_refuses_a_lone_quote() {
        assert_eq!(decode("\""), None);
        assert_eq!(decode("'"), None);
    }

    #[test]
    fn decode_string_reads_lua51_undefined_escapes_as_the_character() {
        // Real 5.1 saves any escaped non-digit as that character, so `\m`,
        // `\x`, `\z`, and `\u` are content there, not escapes.
        let lua51 = LuaVersion::Lua51;
        assert_eq!(decode_in(r#""\m""#, lua51), Some(b"m".to_vec()));
        assert_eq!(decode_in(r#""\x41""#, lua51), Some(b"x41".to_vec()));
        assert_eq!(decode_in(r#""a\z  b""#, lua51), Some(b"az  b".to_vec()));
        assert_eq!(decode_in(r#""\u{48}""#, lua51), Some(b"u{48}".to_vec()));
        // Decimal escapes exist in 5.1 and stay escapes.
        assert_eq!(decode_in(r#""\65""#, lua51), Some(vec![65]));
    }

    #[test]
    fn decode_string_refuses_escapes_the_version_lacks() {
        // `\u{}` arrived in 5.3, and a strict version without it must refuse
        // to fold.
        assert_eq!(decode_in(r#""\u{48}""#, LuaVersion::Lua52), None);
    }

    #[test]
    fn decode_string_reads_long_brackets_without_escapes() {
        assert_eq!(decode("[[hello]]"), Some(b"hello".to_vec()));
        assert_eq!(decode("[==[x]==]"), Some(b"x".to_vec()));
        // A leading newline in a long string is stripped.
        assert_eq!(decode("[[\nhello]]"), Some(b"hello".to_vec()));
        // Long strings don't process escapes.
        assert_eq!(decode("[[a\\nb]]"), Some(b"a\\nb".to_vec()));
    }

    #[test]
    fn decode_string_normalizes_every_long_bracket_eol() {
        // 5.4 manual section 3.1: any EOL sequence converts to a simple
        // newline.
        assert_eq!(decode("[[a\rb]]"), Some(b"a\nb".to_vec()));
        assert_eq!(decode("[[a\r\nb]]"), Some(b"a\nb".to_vec()));
        assert_eq!(decode("[[a\n\rb]]"), Some(b"a\nb".to_vec()));
        // Leading EOL of any form is dropped.
        assert_eq!(decode("[[\r\nhi]]"), Some(b"hi".to_vec()));
        assert_eq!(decode("[[\n\rhi]]"), Some(b"hi".to_vec()));
        assert_eq!(decode("[[\rhi]]"), Some(b"hi".to_vec()));
        // Two CRLFs are two newlines, one stripped.
        assert_eq!(decode("[[\r\n\r\nhi]]"), Some(b"\nhi".to_vec()));
    }

    #[test]
    fn decode_string_keeps_a_lone_cr_in_luau_long_brackets() {
        // Luau's Lexer::fixupMultilineString only recognizes LF and CRLF.
        let luau = LuaVersion::Luau;
        assert_eq!(decode_in("[[a\rb]]", luau), Some(b"a\rb".to_vec()));
        assert_eq!(decode_in("[[a\r\nb]]", luau), Some(b"a\nb".to_vec()));
        assert_eq!(decode_in("[[a\n\rb]]", luau), Some(b"a\n\rb".to_vec()));
        assert_eq!(decode_in("[[\rhi]]", luau), Some(b"\rhi".to_vec()));
    }

    #[test]
    fn decode_string_drops_one_leading_long_bracket_eol_on_every_version() {
        for version in LuaVersion::ALL {
            for (prefix, lua_expected, luau_expected) in [
                ("", "", ""),
                ("\n", "", ""),
                ("\r", "", "\r"),
                ("\r\n", "", ""),
                ("\n\r", "", "\r"),
                ("\n\n", "\n", "\n"),
                ("\r\r", "\n", "\r\r"),
                ("\r\n\r\n", "\n", "\n"),
                ("\n\r\n\r", "\n", "\n\r"),
                ("\n\r\r\n", "\n", "\r\n"),
            ] {
                let expected = if version.is_luau() { luau_expected } else { lua_expected };
                for payload in ["", "payload", "\u{e9}\u{6f22}\u{5b57}"] {
                    for equals in ["", "=="] {
                        let raw = format!("[{equals}[{prefix}{payload}]{equals}]");
                        let decoded = format!("{expected}{payload}");
                        assert_eq!(
                            decode_string(&raw, version).as_deref(),
                            Some(decoded.as_bytes()),
                            "{version:?} {raw:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn encode_string_escapes_quotes_backslashes_and_newlines() {
        assert_eq!(encode_string(b"hello"), r#""hello""#);
        assert_eq!(encode_string(b"a\nb"), r#""a\nb""#);
        assert_eq!(encode_string(b"q\"q"), r#""q\"q""#);
        assert_eq!(encode_string(b"back\\slash"), r#""back\\slash""#);
    }

    #[test]
    fn encode_string_writes_invalid_utf8_as_decimal_escapes() {
        assert_eq!(encode_string(&[0xFF]), r#""\255""#);
    }

    #[test]
    fn encode_string_pads_decimal_escapes_before_a_digit() {
        // `\1` followed by `2` must pad to `\001` so it can't read as `\12`.
        assert_eq!(encode_string(&[0x01, b'2']), r#""\0012""#);
        assert_eq!(encode_string(&[0x01, 0xFF, b'2']), r#""\1\2552""#);
    }

    #[test]
    fn encode_string_round_trips_every_byte_with_and_without_a_following_digit() {
        for byte in 0..=u8::MAX {
            for suffix in [None, Some(b'0'), Some(b'9'), Some(b'x')] {
                let mut bytes = vec![byte];
                bytes.extend(suffix);
                let encoded = encode_string(&bytes);
                for version in LuaVersion::ALL {
                    assert_eq!(
                        decode_string(&encoded, version).as_deref(),
                        Some(bytes.as_slice()),
                        "{bytes:?} {encoded:?} {version:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn encode_string_round_trips_through_decode_string() {
        let cases: &[&[u8]] = &[
            b"hello",
            "\u{e9}\u{6f22}\u{1f600}".as_bytes(),
            "\u{e9}\"\u{6f22}\n\u{1f600}\\".as_bytes(),
            b"a\nb\tc",
            b"quote\"here",
            b"back\\slash",
            &[0xFF, 0xFE, 0x00],
            &[1, 2, 3],
            b"mix\x07\x08\x0c\x0b",
            &[0x01, b'2'],
            &[b'a', 0x01, 0xFF, b'7', 0x7F, 0xC3, b'3', b'"', 0x02],
        ];
        for bytes in cases {
            let encoded = encode_string(bytes);
            assert_eq!(
                decode(&encoded).as_deref(),
                Some(*bytes),
                "round trip failed for {bytes:?} (encoded {encoded:?})"
            );
        }
    }
}
