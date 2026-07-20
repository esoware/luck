//! Lua string/number literal semantics shared by every crate that
//! folds, compares, or re-emits literal values. Working on raw token
//! text instead of decoded values caused an entire class of miscompiles
//! (UTF-8 corruption, escape-state confusion, \"5\" != \"A\").

use crate::LuaVersion;

/// A compile-time Lua number carrying the 5.3+ integer/float subtype.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LuaNumber {
    Int(i64),
    Float(f64),
}

/// Whether the target dialect distinguishes integer and float number
/// subtypes (Lua 5.3+, [`LuaVersion::has_integer_subtype`]) or models every
/// number as a single f64 (5.1, 5.2, Luau). Governs whether
/// [`parse_lua_number`] may return [`LuaNumber::Int`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberSubtypes {
    Unified,
    IntFloat,
}

pub fn parse_lua_number(text: &str, subtypes: NumberSubtypes) -> Option<LuaNumber> {
    if subtypes == NumberSubtypes::Unified {
        return text.parse::<f64>().ok().map(LuaNumber::Float);
    }
    let lower = text.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        if hex.contains('.') || hex.contains('p') {
            // Hex float - rare; not folded.
            return None;
        }
        // Lua 5.3+: hex integer literals wrap into the integer range.
        return u64::from_str_radix(hex, 16)
            .ok()
            .map(|value| LuaNumber::Int(value as i64));
    }
    if lower.contains('.') || lower.contains('e') {
        return lower.parse::<f64>().ok().map(LuaNumber::Float);
    }
    match text.parse::<i64>() {
        Ok(value) => Some(LuaNumber::Int(value)),
        // Overflowing decimal integer literals become floats in 5.3+.
        Err(_) => text.parse::<f64>().ok().map(LuaNumber::Float),
    }
}

/// Decode a string literal token's raw text to its runtime byte value.
/// Handles both quote forms with all Lua escapes and long-bracket strings
/// (EOL normalization, leading-newline strip, no escapes). Returns None
/// on any form this doesn't model exactly - callers must then refuse to
/// fold. Version-dependent: 5.1 treats undefined escapes as the literal
/// character, and Luau's long strings do not treat a lone CR as a newline.
pub fn decode_string_literal(raw: &str, version: LuaVersion) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    if raw.starts_with('[') {
        let after_open = raw.find('[')? + 1;
        let equals = raw[after_open..].chars().take_while(|c| *c == '=').count();
        let content_start = after_open + equals + 1;
        let content_end = raw.len().checked_sub(2 + equals)?;
        if content_start > content_end {
            return Some(Vec::new());
        }
        let content = &raw.as_bytes()[content_start..content_end];
        // Every EOL sequence normalizes to a single newline, and a
        // newline right after the opening bracket is dropped (5.4 §3.1).
        // PUC recognizes CR, LF, CRLF, and LFCR; Luau only LF and CRLF,
        // keeping a lone CR as a literal byte (Lexer::fixupMultilineString).
        let mut out = Vec::with_capacity(content.len());
        let mut idx = 0;
        while idx < content.len() {
            let byte = content[idx];
            let next = content.get(idx + 1).copied();
            match byte {
                b'\r' if next == Some(b'\n') => {
                    out.push(b'\n');
                    idx += 2;
                }
                b'\r' if !version.is_luau() => {
                    out.push(b'\n');
                    idx += 1;
                }
                b'\n' if next == Some(b'\r') && !version.is_luau() => {
                    out.push(b'\n');
                    idx += 2;
                }
                _ => {
                    out.push(byte);
                    idx += 1;
                }
            }
        }
        if out.first() == Some(&b'\n') {
            out.remove(0);
        }
        return Some(out);
    }

    let quote = *bytes.first()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let inner = &bytes[1..bytes.len().checked_sub(1)?];
    let mut out = Vec::with_capacity(inner.len());
    let mut idx = 0;
    while idx < inner.len() {
        let byte = inner[idx];
        if byte != b'\\' {
            out.push(byte);
            idx += 1;
            continue;
        }
        idx += 1;
        let escaped = *inner.get(idx)?;
        idx += 1;
        match escaped {
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0C),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(0x0B),
            b'\\' => out.push(b'\\'),
            b'"' => out.push(b'"'),
            b'\'' => out.push(b'\''),
            b'\n' => {
                out.push(b'\n');
                if !version.is_luau() && inner.get(idx) == Some(&b'\r') {
                    idx += 1;
                }
            }
            b'\r' => {
                out.push(b'\n');
                if inner.get(idx) == Some(&b'\n') {
                    idx += 1;
                }
            }
            b'x' if version.has_hex_escape() => {
                let hi = char::from(*inner.get(idx)?).to_digit(16)?;
                let lo = char::from(*inner.get(idx + 1)?).to_digit(16)?;
                out.push((hi * 16 + lo) as u8);
                idx += 2;
            }
            b'z' if version.has_whitespace_escape() => {
                while inner
                    .get(idx)
                    .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C))
                {
                    idx += 1;
                }
            }
            b'0'..=b'9' => {
                let mut value: u32 = u32::from(escaped - b'0');
                for _ in 0..2 {
                    match inner.get(idx) {
                        Some(digit) if digit.is_ascii_digit() => {
                            value = value * 10 + u32::from(digit - b'0');
                            idx += 1;
                        }
                        _ => break,
                    }
                }
                if value > 255 {
                    return None;
                }
                out.push(value as u8);
            }
            b'u' if version.has_unicode_escape() => {
                if inner.get(idx) != Some(&b'{') {
                    return None;
                }
                idx += 1;
                let mut value: u32 = 0;
                while let Some(&digit) = inner.get(idx) {
                    if digit == b'}' {
                        idx += 1;
                        break;
                    }
                    value = value.checked_mul(16)? + char::from(digit).to_digit(16)?;
                    idx += 1;
                }
                let ch = char::from_u32(value)?;
                let mut encoded = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
            }
            // Lua 5.1 treats any other escaped character as that literal
            // character; under strict versions such tokens cannot exist,
            // so refuse to fold rather than guess.
            _ if !version.has_strict_escapes() => out.push(escaped),
            _ => return None,
        }
    }
    Some(out)
}

/// Encode runtime bytes back into a double-quoted literal, escaping
/// exactly what must be escaped. Valid UTF-8 runs pass through intact;
/// bytes that aren't valid UTF-8 (e.g. produced by `\xFF` escapes)
/// become decimal escapes - never pushed as reinterpreted chars.
pub fn encode_string_literal(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    let push_decimal_escape = |out: &mut String, byte: u8, next_is_digit: bool| {
        // Pad to 3 digits when a digit follows so the escape can't
        // absorb it (`\9` + `9` must not read as `\99`).
        if next_is_digit {
            out.push_str(&format!("\\{byte:03}"));
        } else {
            out.push_str(&format!("\\{byte}"));
        }
    };

    let mut offset = 0usize;
    for chunk in bytes.utf8_chunks() {
        let valid = chunk.valid();
        for (char_offset, ch) in valid.char_indices() {
            let absolute = offset + char_offset;
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\u{7}' => out.push_str("\\a"),
                '\u{8}' => out.push_str("\\b"),
                '\u{C}' => out.push_str("\\f"),
                '\t' => out.push_str("\\t"),
                '\u{B}' => out.push_str("\\v"),
                '\0'..='\u{1F}' | '\u{7F}' => {
                    let next_is_digit = bytes
                        .get(absolute + ch.len_utf8())
                        .is_some_and(|b| b.is_ascii_digit());
                    push_decimal_escape(&mut out, ch as u8, next_is_digit);
                }
                _ => out.push(ch),
            }
        }
        offset += valid.len();
        for (invalid_offset, &byte) in chunk.invalid().iter().enumerate() {
            let next_is_digit = bytes
                .get(offset + invalid_offset + 1)
                .is_some_and(|b| b.is_ascii_digit());
            push_decimal_escape(&mut out, byte, next_is_digit);
        }
        offset += chunk.invalid().len();
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(raw: &str) -> Option<Vec<u8>> {
        decode_string_literal(raw, LuaVersion::Lua54)
    }

    #[test]
    fn number_plain_int_and_float() {
        assert_eq!(
            parse_lua_number("1", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(1))
        );
        assert_eq!(
            parse_lua_number("42", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(42))
        );
        assert_eq!(
            parse_lua_number("1.5", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(1.5))
        );
        assert_eq!(
            parse_lua_number("0.0", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(0.0))
        );
    }

    #[test]
    fn number_exponent_forms() {
        assert_eq!(
            parse_lua_number("1e3", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(1000.0))
        );
        assert_eq!(
            parse_lua_number("2E2", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(200.0))
        );
        assert_eq!(
            parse_lua_number("1.5e-1", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(0.15))
        );
    }

    #[test]
    fn number_without_int_subtype_is_always_float() {
        assert_eq!(
            parse_lua_number("1", NumberSubtypes::Unified),
            Some(LuaNumber::Float(1.0))
        );
        assert_eq!(
            parse_lua_number("42", NumberSubtypes::Unified),
            Some(LuaNumber::Float(42.0))
        );
    }

    #[test]
    fn number_hex_int() {
        assert_eq!(
            parse_lua_number("0x1A", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(26))
        );
        assert_eq!(
            parse_lua_number("0xff", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(255))
        );
        assert_eq!(
            parse_lua_number("0X10", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(16))
        );
    }

    #[test]
    fn number_hex_wraparound_into_i64_range() {
        // Lua 5.3+ hex integers wrap: u64 max reinterprets as -1.
        assert_eq!(
            parse_lua_number("0xffffffffffffffff", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(-1))
        );
        // 2^63 reinterprets as i64::MIN.
        assert_eq!(
            parse_lua_number("0x8000000000000000", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(i64::MIN))
        );
        // Beyond u64 range cannot be folded at all.
        assert_eq!(
            parse_lua_number("0x10000000000000000", NumberSubtypes::IntFloat),
            None
        );
    }

    #[test]
    fn number_hex_floats_rejected() {
        assert_eq!(parse_lua_number("0x1.8p1", NumberSubtypes::IntFloat), None);
        assert_eq!(parse_lua_number("0x1p4", NumberSubtypes::IntFloat), None);
    }

    #[test]
    fn number_decimal_overflow_promotes_to_float() {
        let parsed = parse_lua_number("99999999999999999999", NumberSubtypes::IntFloat);
        assert!(matches!(parsed, Some(LuaNumber::Float(_))));
    }

    #[test]
    fn number_invalid_input() {
        assert_eq!(parse_lua_number("abc", NumberSubtypes::IntFloat), None);
        assert_eq!(parse_lua_number("", NumberSubtypes::IntFloat), None);
        assert_eq!(parse_lua_number("1.2.3", NumberSubtypes::IntFloat), None);
    }

    #[test]
    fn number_int_and_float_are_distinct() {
        assert_ne!(LuaNumber::Int(1), LuaNumber::Float(1.0));
        assert_eq!(
            parse_lua_number("1", NumberSubtypes::IntFloat),
            Some(LuaNumber::Int(1)),
            "1 must fold as an integer under int_subtype"
        );
        assert_eq!(
            parse_lua_number("1.0", NumberSubtypes::IntFloat),
            Some(LuaNumber::Float(1.0)),
            "1.0 must fold as a float"
        );
    }

    #[test]
    fn decode_simple_string() {
        assert_eq!(decode(r#""hello""#), Some(b"hello".to_vec()));
        assert_eq!(decode("'hi'"), Some(b"hi".to_vec()));
        assert_eq!(decode(r#""""#), Some(Vec::new()));
    }

    #[test]
    fn decode_escape_classes() {
        assert_eq!(decode(r#""\n""#), Some(vec![b'\n']));
        assert_eq!(decode(r#""\t""#), Some(vec![b'\t']));
        assert_eq!(decode(r#""\\""#), Some(vec![b'\\']));
        assert_eq!(decode(r#""\"""#), Some(vec![b'"']));
        assert_eq!(decode(r#""\a""#), Some(vec![0x07]));
        assert_eq!(decode(r#""\r""#), Some(vec![b'\r']));
    }

    #[test]
    fn decode_hex_escape() {
        assert_eq!(decode(r#""\x41""#), Some(vec![0x41]));
        assert_eq!(decode(r#""\xff""#), Some(vec![0xFF]));
    }

    #[test]
    fn decode_decimal_escape() {
        assert_eq!(decode(r#""\65""#), Some(vec![65]));
        assert_eq!(decode(r#""\9""#), Some(vec![9]));
        assert_eq!(decode(r#""\255""#), Some(vec![255]));
    }

    #[test]
    fn decode_decimal_escape_out_of_range() {
        assert_eq!(decode(r#""\256""#), None);
        assert_eq!(decode(r#""\300""#), None);
    }

    #[test]
    fn decode_unicode_escape() {
        assert_eq!(decode(r#""\u{48}""#), Some(b"H".to_vec()));
        // U+00E9 (e-acute) encodes to two UTF-8 bytes.
        assert_eq!(decode(r#""\u{e9}""#), Some(vec![0xC3, 0xA9]));
    }

    #[test]
    fn decode_z_line_continuation() {
        // `\z` skips the whitespace run that follows, including newlines.
        assert_eq!(decode("\"a\\z   \n   b\""), Some(b"ab".to_vec()));
    }

    #[test]
    fn decode_bad_escape_is_none() {
        assert_eq!(decode(r#""\q""#), None);
        assert_eq!(decode_string_literal(r#""\q""#, LuaVersion::Luau), None);
    }

    #[test]
    fn decode_lua51_lax_escapes_are_literal() {
        // Real 5.1 saves any escaped non-digit as that character, so
        // \m, \x, \z, \u are content, not escapes (5.2 §8.1 tightened this).
        let lua51 = LuaVersion::Lua51;
        assert_eq!(decode_string_literal(r#""\m""#, lua51), Some(b"m".to_vec()));
        assert_eq!(
            decode_string_literal(r#""\x41""#, lua51),
            Some(b"x41".to_vec())
        );
        assert_eq!(
            decode_string_literal(r#""a\z  b""#, lua51),
            Some(b"az  b".to_vec())
        );
        assert_eq!(
            decode_string_literal(r#""\u{48}""#, lua51),
            Some(b"u{48}".to_vec())
        );
        // Decimal escapes exist in 5.1 and stay escapes.
        assert_eq!(decode_string_literal(r#""\65""#, lua51), Some(vec![65]));
    }

    #[test]
    fn decode_version_gated_escapes_refuse_fold() {
        // \u{} does not exist before 5.3; a strict version that lacks the
        // escape must refuse to fold rather than guess.
        assert_eq!(
            decode_string_literal(r#""\u{48}""#, LuaVersion::Lua52),
            None
        );
    }

    #[test]
    fn decode_long_bracket_eol_normalization() {
        // 5.4 §3.1: any EOL sequence converts to a simple newline.
        let lua54 = LuaVersion::Lua54;
        assert_eq!(
            decode_string_literal("[[a\rb]]", lua54),
            Some(b"a\nb".to_vec())
        );
        assert_eq!(
            decode_string_literal("[[a\r\nb]]", lua54),
            Some(b"a\nb".to_vec())
        );
        assert_eq!(
            decode_string_literal("[[a\n\rb]]", lua54),
            Some(b"a\nb".to_vec())
        );
        // Leading EOL of any form is dropped.
        assert_eq!(
            decode_string_literal("[[\r\nhi]]", lua54),
            Some(b"hi".to_vec())
        );
        assert_eq!(
            decode_string_literal("[[\n\rhi]]", lua54),
            Some(b"hi".to_vec())
        );
        assert_eq!(
            decode_string_literal("[[\rhi]]", lua54),
            Some(b"hi".to_vec())
        );
        // Two CRLFs are two newlines, one stripped.
        assert_eq!(
            decode_string_literal("[[\r\n\r\nhi]]", lua54),
            Some(b"\nhi".to_vec())
        );
    }

    #[test]
    fn decode_long_bracket_eol_luau_lone_cr_is_content() {
        // Luau's Lexer::fixupMultilineString only recognizes LF and CRLF.
        let luau = LuaVersion::Luau;
        assert_eq!(
            decode_string_literal("[[a\rb]]", luau),
            Some(b"a\rb".to_vec())
        );
        assert_eq!(
            decode_string_literal("[[a\r\nb]]", luau),
            Some(b"a\nb".to_vec())
        );
        assert_eq!(
            decode_string_literal("[[a\n\rb]]", luau),
            Some(b"a\n\rb".to_vec())
        );
        assert_eq!(
            decode_string_literal("[[\rhi]]", luau),
            Some(b"\rhi".to_vec())
        );
    }

    #[test]
    fn decode_long_bracket_string() {
        assert_eq!(decode("[[hello]]"), Some(b"hello".to_vec()));
        assert_eq!(decode("[==[x]==]"), Some(b"x".to_vec()));
        // A leading newline in a long string is stripped.
        assert_eq!(decode("[[\nhello]]"), Some(b"hello".to_vec()));
        // Long strings do not process escapes.
        assert_eq!(decode("[[a\\nb]]"), Some(b"a\\nb".to_vec()));
    }

    #[test]
    fn encode_simple_and_escapes() {
        assert_eq!(encode_string_literal(b"hello"), r#""hello""#);
        assert_eq!(encode_string_literal(b"a\nb"), r#""a\nb""#);
        assert_eq!(encode_string_literal(b"q\"q"), r#""q\"q""#);
        assert_eq!(encode_string_literal(b"back\\slash"), r#""back\\slash""#);
    }

    #[test]
    fn encode_non_utf8_byte_becomes_decimal_escape() {
        assert_eq!(encode_string_literal(&[0xFF]), r#""\255""#);
    }

    #[test]
    fn encode_pads_when_digit_follows() {
        // `\1` followed by `2` must pad to `\001` so it can't read as `\12`.
        assert_eq!(encode_string_literal(&[0x01, b'2']), r#""\0012""#);
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let cases: &[&[u8]] = &[
            b"hello",
            b"a\nb\tc",
            b"quote\"here",
            b"back\\slash",
            &[0xFF, 0xFE, 0x00],
            &[1, 2, 3],
            b"mix\x07\x08\x0c\x0b",
            &[0x01, b'2'],
        ];
        for bytes in cases {
            let encoded = encode_string_literal(bytes);
            assert_eq!(
                decode(&encoded).as_deref(),
                Some(*bytes),
                "round trip failed for {bytes:?} (encoded {encoded:?})"
            );
        }
    }
}
