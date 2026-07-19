use luck_token::*;

use crate::common::first_kind;

#[test]
fn number_integer() {
    assert_eq!(first_kind("42"), TokenKind::Number("42".into()));
}

#[test]
fn number_float() {
    assert_eq!(first_kind("3.14"), TokenKind::Number("3.14".into()));
}

#[test]
fn number_leading_dot() {
    assert_eq!(first_kind(".5"), TokenKind::Number(".5".into()));
}

#[test]
fn number_exponent() {
    assert_eq!(first_kind("1e10"), TokenKind::Number("1e10".into()));
}

#[test]
fn number_exponent_negative() {
    assert_eq!(first_kind("3.14e-2"), TokenKind::Number("3.14e-2".into()));
}

#[test]
fn number_exponent_positive() {
    assert_eq!(first_kind("1E+5"), TokenKind::Number("1E+5".into()));
}

#[test]
fn number_hex_lower() {
    assert_eq!(first_kind("0xFF"), TokenKind::Number("0xFF".into()));
}

#[test]
fn number_hex_upper() {
    assert_eq!(first_kind("0XAB"), TokenKind::Number("0XAB".into()));
}

#[test]
fn number_adjacent_to_concat_is_malformed() {
    // Every real Lua numeral scanner consumes the dots, so `1..2` is
    // "malformed number", never `1 .. 2`.
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau] {
        for source in ["return 1..2", "return 1.5..2", "return 1e5..2"] {
            let result = luck_lexer::lex(source, version);
            assert!(
                !result.errors.is_empty(),
                "{source} must be malformed under {version:?}"
            );
        }
    }
    // With a space the concat is valid.
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau] {
        let result = luck_lexer::lex("return 1 ..2", version);
        assert!(result.errors.is_empty(), "spaced concat lexes: {version:?}");
    }
}

#[test]
fn hex_number_adjacent_to_concat_per_version() {
    // 5.1's hex scanner stops at the dot: `0xFF..2` is a valid concat
    // there. Luau's scanner consumes the dots: malformed. 5.2+ fails
    // in the hex-float path.
    assert!(
        luck_lexer::lex("return 0xFF..2", LuaVersion::Lua51)
            .errors
            .is_empty()
    );
    assert!(
        !luck_lexer::lex("return 0xFF..2", LuaVersion::Luau)
            .errors
            .is_empty()
    );
    assert!(
        !luck_lexer::lex("return 0xFF..2", LuaVersion::Lua54)
            .errors
            .is_empty()
    );
    assert!(
        !luck_lexer::lex("return 0b1..2", LuaVersion::Luau)
            .errors
            .is_empty()
    );
}
