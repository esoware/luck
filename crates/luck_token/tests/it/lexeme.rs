use luck_token::{
    LuaVersion, is_interpolated_text, is_numeral, is_numeral_in, is_string_literal,
    is_string_literal_in,
};

#[test]
fn numerals_follow_their_dialect() {
    for text in ["0", "42", "3.", ".5", "1e10", "1E+5", "3.14e-2", "0xFF", "0X1f", "1.e5"] {
        assert!(LuaVersion::ALL.iter().all(|&version| is_numeral_in(text, version)), "{text}");
    }
    for (text, dialect_count) in [
        ("0x1.8p3", 4),
        ("0x.8", 4),
        ("0b101", 1),
        ("1_000", 1),
        ("0_x1", 1),
        ("1i", 1),
        ("0xffffffffffffffffi", 1),
        ("1i__", 1),
    ] {
        let accepting = LuaVersion::ALL.iter().filter(|&&v| is_numeral_in(text, v)).count();
        assert_eq!(accepting, dialect_count, "{text}");
        assert!(is_numeral(text), "{text}");
    }
}

#[test]
fn a_text_that_is_not_one_numeral_is_refused() {
    for text in [
        "",
        "-1",
        "+1",
        "x",
        "1x",
        "1 ",
        " 1",
        "1..2",
        "1.5.2",
        "0x",
        "0x.",
        "1e",
        "1e+",
        ".",
        "..5",
        ".0x1",
        "2.5i",
        "1e5i",
        "9223372036854775808i",
        "0b",
        "0b2",
        "inf",
        "_1",
    ] {
        assert!(!is_numeral(text), "{text}");
    }
}

#[test]
fn string_literals_follow_their_dialect() {
    for text in ["\"\"", "''", "\"a'b\"", "'a\\'b'", "\"\\n\\\\\"", "[[]]", "[==[a]]b]==]"] {
        assert!(
            LuaVersion::ALL.iter().all(|&version| is_string_literal_in(text, version)),
            "{text}"
        );
    }
    // Lua 5.1 reads an unknown escape as the character, 5.2+ reject it,
    // and only 5.3+ and Luau read `\u{...}`.
    assert!(is_string_literal_in("\"\\q\"", LuaVersion::Lua51));
    assert!(!is_string_literal_in("\"\\q\"", LuaVersion::Lua54));
    assert!(!is_string_literal_in("\"\\u{110000}\"", LuaVersion::Lua53));
    assert!(is_string_literal_in("\"\\u{110000}\"", LuaVersion::Lua54));
    assert!(!is_string_literal_in("[[a[[b]]", LuaVersion::Lua51));
    assert!(is_string_literal_in("[[a[[b]]", LuaVersion::Lua54));
    assert!(is_string_literal_in("\"\\z\n x\"", LuaVersion::Lua52));
    assert!(!is_string_literal_in("\"a\0b\"", LuaVersion::Luau));
    assert!(is_string_literal_in("\"a\0b\"", LuaVersion::Lua54));
}

#[test]
fn a_text_that_is_not_one_string_literal_is_refused() {
    for text in [
        "",
        "abc",
        "\"",
        "\"abc",
        "abc\"",
        "'a\"",
        "\"a\"b\"",
        "\"a\nb\"",
        "\"a\rb\"",
        "\"a\\\"",
        "\"\\256\"",
        "[[a",
        "[[a]]]",
        "[=[a]]",
        "[=[a]==]",
        "[a]",
    ] {
        assert!(!is_string_literal(text), "{text:?}");
    }
}

#[test]
fn interpolated_text_takes_luau_escapes_and_no_delimiters() {
    for text in ["", "abc", "a}b", "\\{", "\\}", "\\`", "\\n", "\\u{41}", "\\x41", "\\q"] {
        assert!(is_interpolated_text(text), "{text:?}");
    }
    for text in ["{", "a{b", "`", "a\nb", "a\rb", "\\", "a\\", "\\x4", "\\256", "a\0"] {
        assert!(!is_interpolated_text(text), "{text:?}");
    }
}
