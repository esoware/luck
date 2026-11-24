use luck_token::{
    EscapeError, LuaVersion, StringValue, decode_interpolated_text, decode_string, read_escape,
    spell_string, spell_string_shorter_than, write_interpolated_text, write_long_string,
};

fn decode(raw: &str) -> Vec<u8> {
    decode_in(raw, LuaVersion::Lua54)
}

fn decode_in(raw: &str, version: LuaVersion) -> Vec<u8> {
    let mut output = Vec::new();
    match decode_string(raw.as_bytes(), true, version, &mut output) {
        StringValue::Verbatim(bytes) => bytes.to_vec(),
        StringValue::Decoded => output,
    }
}

/// The spelling of `bytes`, checked to be UTF-8 and as long as it said it
/// would be.
fn written(bytes: &[u8]) -> String {
    let spelling = spell_string(bytes);
    let mut written_bytes = Vec::new();
    spelling.write_to(&mut written_bytes);
    let output = String::from_utf8(written_bytes).expect("a spelling is UTF-8");
    assert_eq!(output.len(), spelling.length(), "{output:?}");
    assert_eq!(output.starts_with('['), spelling.is_long_bracket(), "{output:?}");
    output
}

fn escape_error(escape: &str, version: LuaVersion) -> (usize, Option<EscapeError>) {
    let (length, meaning) = read_escape(escape.as_bytes(), false, version);
    (length, meaning.err())
}

#[test]
fn a_body_with_nothing_to_rewrite_is_its_own_value() {
    for raw in [
        br#""hello""#,
        "'a\u{e9}b'".as_bytes(),
        b"'a\xE9b'",
        b"[[a\nb]]",
        b"[==[\nhi]==]",
        b"[[\xFF]]",
        b"[[]]",
        b"\"\"",
    ] {
        for version in LuaVersion::ALL {
            let mut output = Vec::new();
            let value = decode_string(raw, true, version, &mut output);
            assert!(matches!(value, StringValue::Verbatim(_)), "{raw:?} {version:?}");
            assert!(output.is_empty(), "{raw:?} {version:?}");
        }
    }
    for raw in [&br#""a\n""#[..], b"[[a\r\nb]]", b"[[\r\na\r\n]]"] {
        let mut output = Vec::new();
        assert_eq!(decode_string(raw, true, LuaVersion::Lua54, &mut output), StringValue::Decoded);
    }
}

#[test]
fn decodes_every_kind_of_escape() {
    for (raw, expected) in [
        (r#""\n\t\\\"\a\r\b\f\v""#, &b"\n\t\\\"\x07\r\x08\x0C\x0B"[..]),
        ("'\\''", b"'"),
        (r#""\x41\xff""#, b"A\xFF"),
        (r#""\65\9\255\0012""#, b"A\x09\xFF\x012"),
        (r#""\u{48}\u{e9}""#, "H\u{e9}".as_bytes()),
        ("\"a\\z   \n   b\"", b"ab"),
        ("\"a\\\nb\"", b"a\nb"),
        ("\"a\\\r\nb\"", b"a\nb"),
        ("\"a\\\n\rb\"", b"a\nb"),
    ] {
        assert_eq!(decode(raw), expected, "{raw}");
    }
}

#[test]
fn writes_unicode_escapes_in_lua_s_longer_utf8_forms() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Luau] {
        assert_eq!(decode_in(r#""\u{D800}""#, version), [0xED, 0xA0, 0x80], "{version:?}");
    }
    assert_eq!(decode(r#""\u{110000}""#), [0xF4, 0x90, 0x80, 0x80]);
    assert_eq!(decode(r#""\u{200000}""#), [0xF8, 0x88, 0x80, 0x80, 0x80]);
    assert_eq!(decode(r#""\u{7FFFFFFF}""#), [0xFD, 0xBF, 0xBF, 0xBF, 0xBF, 0xBF]);
}

#[test]
fn lua51_and_luau_read_an_undefined_escape_as_the_character() {
    let lua51 = LuaVersion::Lua51;
    assert_eq!(decode_in(r#""\m""#, lua51), b"m");
    assert_eq!(decode_in(r#""\x41""#, lua51), b"x41");
    assert_eq!(decode_in(r#""a\z  b""#, lua51), b"az  b");
    assert_eq!(decode_in(r#""\u{48}""#, lua51), b"u{48}");
    assert_eq!(decode_in(r#""\65""#, lua51), b"A");
    assert_eq!(decode_in(r#""\q\{""#, LuaVersion::Luau), b"q{");
    let (_, lenient) = read_escape(b"q", false, lua51);
    assert!(lenient.is_ok_and(luck_token::Escaped::is_lenient));
    let (_, defined) = read_escape(b"n", false, lua51);
    assert!(defined.is_ok_and(|escaped| !escaped.is_lenient()));
}

#[test]
fn an_interpolated_string_also_escapes_its_delimiters() {
    let mut output = Vec::new();
    let value = decode_interpolated_text(br"a\{b\}c\`d\q", true, LuaVersion::Luau, &mut output);
    assert_eq!(value, StringValue::Decoded);
    assert_eq!(output, b"a{b}c`dq");
    let (_, brace) = read_escape(b"{", true, LuaVersion::Luau);
    assert!(brace.is_ok_and(|escaped| !escaped.is_lenient()));
}

#[test]
fn reports_each_malformed_escape_and_how_far_it_reached() {
    let lua54 = LuaVersion::Lua54;
    assert_eq!(escape_error("256", lua54), (3, Some(EscapeError::DecimalTooLarge(256))));
    assert_eq!(escape_error("x4g", lua54), (2, Some(EscapeError::HexDigits(1))));
    assert_eq!(escape_error("xg", lua54), (1, Some(EscapeError::HexDigits(0))));
    assert_eq!(escape_error("u41", lua54), (1, Some(EscapeError::UnicodeBrace)));
    assert_eq!(escape_error("u{}", lua54), (2, Some(EscapeError::UnicodeDigits)));
    assert_eq!(escape_error("u{80000000}", lua54), (10, Some(EscapeError::UnicodeTooLarge)));
    assert_eq!(escape_error("u{41", lua54), (4, Some(EscapeError::UnicodeUnclosed)));
    assert_eq!(escape_error("q", lua54), (1, Some(EscapeError::Undefined)));
    assert_eq!(
        escape_error("u{110000}", LuaVersion::Lua53),
        (8, Some(EscapeError::UnicodeTooLarge))
    );
    let seventeen_digits = "u{00000000000000041}";
    assert_eq!(
        escape_error(seventeen_digits, LuaVersion::Luau).1,
        Some(EscapeError::UnicodeTooLarge)
    );
    assert_eq!(escape_error(seventeen_digits, lua54), (20, None));
    assert_eq!(escape_error("u{48}", LuaVersion::Lua52), (1, Some(EscapeError::Undefined)));
}

#[test]
fn a_malformed_escape_puts_nothing_and_the_string_goes_on() {
    assert_eq!(decode(r#""a\256b""#), b"ab");
    assert_eq!(decode(r#""a\u{}b""#), b"a}b");
}

#[test]
fn a_literal_cut_off_before_its_closer_holds_what_it_reached() {
    for (raw, expected) in
        [("\"abc", &b"abc"[..]), ("\"a'", b"a'"), ("\"abc\\", b"abc"), ("[[ab", b"ab"), ("\"", b"")]
    {
        assert_eq!(decode(raw), expected, "{raw:?}");
    }
}

#[test]
fn long_brackets_turn_every_line_break_into_a_newline() {
    assert_eq!(decode("[[hello]]"), b"hello");
    assert_eq!(decode("[==[x]==]"), b"x");
    assert_eq!(decode("[[a\\nb]]"), b"a\\nb");
    for (raw, expected) in [
        ("[[a\rb]]", &b"a\nb"[..]),
        ("[[a\r\nb]]", b"a\nb"),
        ("[[a\n\rb]]", b"a\nb"),
        ("[[\r\nhi]]", b"hi"),
        ("[[\n\rhi]]", b"hi"),
        ("[[\rhi]]", b"hi"),
        ("[[\r\n\r\nhi]]", b"\nhi"),
    ] {
        assert_eq!(decode(raw), expected, "{raw:?}");
    }
}

#[test]
fn luau_long_brackets_keep_a_lone_cr() {
    let luau = LuaVersion::Luau;
    assert_eq!(decode_in("[[a\rb]]", luau), b"a\rb");
    assert_eq!(decode_in("[[a\r\nb]]", luau), b"a\nb");
    assert_eq!(decode_in("[[a\n\rb]]", luau), b"a\n\rb");
    assert_eq!(decode_in("[[\rhi]]", luau), b"\rhi");
}

#[test]
fn writes_the_shortest_spelling() {
    for (bytes, expected) in [
        (&b"hello"[..], r#""hello""#),
        (b"", r#""""#),
        (b"it's", r#""it's""#),
        (b"say \"hi\"", r#"'say "hi"'"#),
        (b"a\nb", r#""a\nb""#),
        (b"\\\\\\", "[[\\\\\\]]"),
        (b"a\nb\nc\nd", "[[a\nb\nc\nd]]"),
        (b"\n\n\n\n", "[[\n\n\n\n\n]]"),
        (b"]]\n\n\n\n\n", "[=[]]\n\n\n\n\n]=]"),
        (b"\x01\x02", r#""\1\2""#),
        (b"\x012", r#""\0012""#),
        (b"\xFF7", r#""\2557""#),
        (b"tab\there", r#""tab\there""#),
        (b"\x7F", r#""\127""#),
    ] {
        assert_eq!(written(bytes), expected, "{bytes:?}");
    }
}

#[test]
fn spells_a_string_only_shorter_than_a_length() {
    let escapes = b"\x01\"'\\".repeat(1000);
    for bytes in [
        &b"hello"[..],
        b"",
        b"a\nb\nc\nd",
        b"]]\n\n\n\n\n",
        b"\x01\x02",
        &escapes,
        b"\xFF\xFE",
        "\u{e9}\u{6f22}\\\\\\\\".as_bytes(),
    ] {
        let spelling = written(bytes);
        for length in 0..spelling.len() + 3 {
            let shorter = spell_string_shorter_than(bytes, length).map(|shorter| {
                let mut written_bytes = Vec::new();
                shorter.write_to(&mut written_bytes);
                assert_eq!(written_bytes.len(), shorter.length(), "{bytes:?}");
                written_bytes
            });
            let expected = (spelling.len() < length).then(|| spelling.as_bytes().to_vec());
            assert_eq!(shorter, expected, "{bytes:?} shorter than {length}");
        }
    }
}

#[test]
fn every_written_string_reads_back_as_itself() {
    let mut cases: Vec<Vec<u8>> = (0..=u8::MAX)
        .flat_map(|byte| [vec![byte], vec![byte, b'0'], vec![byte, b'9', b'x'], vec![b'"', byte]])
        .collect();
    cases.extend(
        [
            &b"\\\\\\\"\"\"''"[..],
            b"]]]]\n\n\n\n\n\n[[",
            b"]=]\\\\\\\\\\\\]==",
            "\u{e9}\u{6f22}\n\u{1f600}\\\\\\\\".as_bytes(),
            b"\n\\\\\\\\\\\\\\",
            &[0xFF, 0xFE, 0x00],
            &[b'a', 0x01, 0xFF, b'7', 0x7F, 0xC3, b'3', b'"', 0x02],
        ]
        .map(<[u8]>::to_vec),
    );
    for bytes in cases {
        let text = written(&bytes);
        for version in LuaVersion::ALL {
            assert_eq!(decode_in(&text, version), bytes, "{text:?} {version:?}");
        }
        assert_eq!(written(&decode(&text)), text);
    }
}

#[test]
fn long_strings_take_the_lowest_level_no_closer_in_the_text_takes() {
    for (text, expected) in [
        ("plain", "[[plain]]"),
        ("]]", "[=[]]]=]"),
        ("]=]", "[==[]=]]==]"),
        ("]] ]=] ]===]", "[==[]] ]=] ]===]]==]"),
        ("a]=", "[[a]=]]"),
        ("x]", "[=[x]]=]"),
        ("=]", "[=[=]]=]"),
        ("[[ ]=]", "[==[[[ ]=]]==]"),
        ("\nline", "[[\n\nline]]"),
    ] {
        let mut output = Vec::new();
        assert!(write_long_string(text, &mut output), "{text:?}");
        assert_eq!(output, expected.as_bytes(), "{text:?}");
        for version in LuaVersion::ALL {
            assert_eq!(decode_in(expected, version), text.as_bytes(), "{expected:?} {version:?}");
        }
    }
    let mut output = Vec::new();
    assert!(!write_long_string("a\r\nb", &mut output));
    assert!(!write_long_string("a\0b", &mut output));
    assert_eq!(output, b"");
}

#[test]
fn a_long_string_takes_the_level_past_a_closer_at_every_level() {
    let levels = 300;
    let mut text = String::new();
    for level in 0..levels {
        text.push(']');
        text.push_str(&"=".repeat(level));
        text.push(']');
    }
    let mut output = Vec::new();
    assert!(write_long_string(&text, &mut output));
    let equals = "=".repeat(levels);
    assert_eq!(output, format!("[{equals}[{text}]{equals}]").as_bytes());
}

#[test]
fn interpolated_text_escapes_what_its_lexer_reads() {
    let mut output = Vec::new();
    write_interpolated_text(b"a{b}`c\\d\"e\n\xFF\x017", &mut output);
    assert_eq!(output, br#"a\{b}\`c\\d"e\n\255\0017"#);
    let mut decoded = Vec::new();
    let value = decode_interpolated_text(&output, true, LuaVersion::Luau, &mut decoded);
    assert_eq!(value, StringValue::Decoded);
    assert_eq!(decoded, b"a{b}`c\\d\"e\n\xFF\x017");
}
