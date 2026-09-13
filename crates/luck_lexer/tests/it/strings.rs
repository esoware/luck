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
fn short_string_scan_boundaries_preserve_spelling_and_errors() {
    for length in [0, 1, 7, 8, 9, 15, 16, 31, 32, 33, 127, 128, 1024] {
        for quote in ['\'', '"'] {
            let opposite = if quote == '\'' { '"' } else { '\'' };
            let text = format!("{}\u{e9}{opposite}\u{6f22}", "a".repeat(length));
            for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau] {
                let source = format!("{quote}{text}\\n{text}{quote}");
                let result = luck_lexer::lex(&source, version);
                assert!(result.errors.is_empty(), "{source:?}: {:?}", result.errors);
                assert_eq!(
                    result.tokens[0].kind,
                    TokenKind::StringLiteral(source.as_str().into())
                );
                assert_eq!(result.tokens[0].span, Span::new(0, source.len() as u32));
                for newline in ["\n", "\r", "\r\n"] {
                    let source = format!("{quote}{text}{newline}{text}{quote}");
                    let result = luck_lexer::lex(&source, version);
                    assert!(
                        result
                            .errors
                            .iter()
                            .any(|error| error.message == "unterminated string"),
                        "{source:?}: {:?}",
                        result.errors
                    );
                }
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
