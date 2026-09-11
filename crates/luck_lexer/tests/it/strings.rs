use luck_token::*;

use crate::common::{first_kind, kinds_v, lex51};

#[test]
fn string_double_quoted() {
    assert_eq!(
        first_kind("\"hello\""),
        TokenKind::StringLiteral("\"hello\"".into())
    );
}

#[test]
fn string_single_quoted() {
    assert_eq!(
        first_kind("'hello'"),
        TokenKind::StringLiteral("'hello'".into())
    );
}

#[test]
fn string_escape_sequences() {
    let cases = [
        ("\"\\a\"", "\"\\a\""),
        ("\"\\b\"", "\"\\b\""),
        ("\"\\f\"", "\"\\f\""),
        ("\"\\n\"", "\"\\n\""),
        ("\"\\r\"", "\"\\r\""),
        ("\"\\t\"", "\"\\t\""),
        ("\"\\v\"", "\"\\v\""),
        ("\"\\\\\"", "\"\\\\\""),
        ("\"\\\"\"", "\"\\\"\""),
        ("'\\'x'", "'\\'x'"),
    ];
    for (source, expected_literal) in cases {
        assert_eq!(
            first_kind(source),
            TokenKind::StringLiteral(expected_literal.into()),
            "failed for: {:?}",
            source
        );
    }
}

#[test]
fn string_escape_decimal() {
    // \065 = 'A'
    assert_eq!(
        first_kind("\"\\065\""),
        TokenKind::StringLiteral("\"\\065\"".into())
    );
}

#[test]
fn string_line_continuations_all_versions() {
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        for quote in ['\'', '"'] {
            for newline in ["\r\n", "\n", "\r"] {
                let source = format!("{quote}a \\{newline}b{quote}");
                assert_eq!(
                    kinds_v(&source, version),
                    vec![TokenKind::StringLiteral(source.as_str().into())],
                    "{version:?} {source:?}"
                );
            }
        }
    }
}

#[test]
fn long_string_levels() {
    let cases = [
        ("[[text]]", "[[text]]"),
        ("[=[text]=]", "[=[text]=]"),
        ("[==[text]==]", "[==[text]==]"),
        ("[===[text]===]", "[===[text]===]"),
    ];
    for (source, expected) in cases {
        assert_eq!(
            first_kind(source),
            TokenKind::StringLiteral(expected.into()),
            "failed for: {:?}",
            source
        );
    }
}

#[test]
fn long_string_with_newlines() {
    let src = "[[line1\nline2]]";
    assert_eq!(first_kind(src), TokenKind::StringLiteral(src.into()));
}

#[test]
fn string_decimal_escape_255() {
    let result = lex51("\"\\255\"");
    assert!(result.errors.is_empty());
    assert_eq!(
        result.tokens[0].kind,
        TokenKind::StringLiteral("\"\\255\"".into())
    );
}
