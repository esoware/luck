use alloc::borrow::Cow;

use luck_token::{
    LuaVersion, decode_string, encode_string, short_string_body, unicode_escape_codepoint,
};

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

#[test]
fn unicode_escape_codepoint_applies_each_version_range() {
    assert_eq!(unicode_escape_codepoint(b"10FFFF", LuaVersion::Lua53), Some(0x10_FFFF));
    assert_eq!(unicode_escape_codepoint(b"110000", LuaVersion::Lua53), None);
    assert_eq!(unicode_escape_codepoint(b"110000", LuaVersion::Luau), None);
    assert_eq!(unicode_escape_codepoint(b"7FFFFFFF", LuaVersion::Lua54), Some(0x7FFF_FFFF));
    assert_eq!(unicode_escape_codepoint(b"80000000", LuaVersion::Lua54), None);
    assert_eq!(unicode_escape_codepoint(b"", LuaVersion::Lua54), None);
    let seventeen_digits = b"00000000000000041";
    assert_eq!(unicode_escape_codepoint(seventeen_digits, LuaVersion::Luau), None);
    assert_eq!(unicode_escape_codepoint(seventeen_digits, LuaVersion::Lua54), Some(0x41));
}

#[test]
fn decode_string_refuses_an_empty_unicode_escape() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        assert_eq!(
            decode_in(r#""\u{}""#, version),
            None,
            "{version:?} rejects `\\u{{}}` with no digits, so it must not decode to a NUL byte"
        );
    }
}

#[test]
fn decode_string_refuses_an_unterminated_unicode_escape() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        assert_eq!(
            decode_in(r#""\u{41""#, version),
            None,
            "{version:?} requires the closing `}}` of a `\\u` escape, so `\\u{{41` must not decode"
        );
    }
}

#[test]
fn decode_string_refuses_luau_unicode_escapes_past_sixteen_digits() {
    assert_eq!(
        decode_in(r#""\u{00000000000000041}""#, LuaVersion::Luau),
        None,
        "Lexer::fixupQuotedString reads at most 16 digits, so Luau rejects a 17-digit `\\u` escape"
    );
}

#[test]
fn decode_string_refuses_text_without_its_closing_quote() {
    for raw in ["\"a'", "'a\"", "\"abc", "\"abc\\"] {
        assert_eq!(decode(raw), None, "{raw:?} has no matching closing quote");
    }
}

#[test]
fn decode_string_refuses_malformed_long_brackets() {
    for raw in ["[abc]", "[[", "[[x]", "[=[x]]"] {
        assert_eq!(decode(raw), None, "{raw:?} is not a complete long-bracket string");
    }
}
