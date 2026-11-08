use luck_lexer::lex;
use luck_token::{LuaVersion, SourceError, Span, TokenKind};

use crate::common::{first_kind, kinds_in, lex_lua51};

#[test]
fn quoted_strings_keep_their_spelling() {
    for source in ["\"hello\"", "'hello'"] {
        assert_eq!(first_kind(source), TokenKind::StringLiteral(source.into()));
    }
}

#[test]
fn single_char_escapes_keep_their_spelling() {
    for source in [
        "\"\\a\"", "\"\\b\"", "\"\\f\"", "\"\\n\"", "\"\\r\"", "\"\\t\"", "\"\\v\"", "\"\\\\\"",
        "\"\\\"\"", "'\\'x'",
    ] {
        assert_eq!(
            first_kind(source),
            TokenKind::StringLiteral(source.into()),
            "failed for: {source:?}"
        );
    }
}

#[test]
fn decimal_escapes_up_to_255_lex() {
    // \065 = 'A'
    for source in ["\"\\065\"", "\"\\255\""] {
        let result = lex_lua51(source);
        assert_eq!(result.errors, Vec::<SourceError>::new());
        assert_eq!(result.tokens[0].kind, TokenKind::StringLiteral(source.into()));
    }
}

#[test]
fn decimal_escape_over_255_is_error() {
    let result = lex_lua51("\"\\256\"");
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("too large"));
}

#[test]
fn string_line_continuations_all_versions() {
    for version in LuaVersion::ALL {
        for quote in ['\'', '"'] {
            for newline in ["\r\n", "\n", "\r"] {
                let source = format!("{quote}a \\{newline}b{quote}");
                assert_eq!(
                    kinds_in(&source, version),
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
                let result = lex(&source, version);
                assert!(result.errors.is_empty(), "{source:?}: {:?}", result.errors);
                assert_eq!(result.tokens[0].kind, TokenKind::StringLiteral(source.as_str().into()));
                assert_eq!(result.tokens[0].span, Span::from_indices(0, source.len()));
                for newline in ["\n", "\r", "\r\n"] {
                    let broken_source = format!("{quote}{text}{newline}{text}{quote}");
                    let broken_result = lex(&broken_source, version);
                    assert!(
                        broken_result
                            .errors
                            .iter()
                            .any(|error| error.message == "unterminated string"),
                        "{broken_source:?}: {:?}",
                        broken_result.errors
                    );
                }
            }
        }
    }
}

#[test]
fn unterminated_string_is_error() {
    let result = lex_lua51("\"hello");
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn undefined_escape_is_error_outside_lua51() {
    // 5.2+ and Luau reject undefined escapes; real 5.1 accepts them as
    // the literal character.
    for version in [LuaVersion::Lua52, LuaVersion::Luau] {
        let result = lex("\"\\q\"", version);
        assert!(!result.errors.is_empty(), "{version:?} must reject \\q");
        assert!(result.errors[0].message.contains("invalid escape"));
    }
    let result = lex_lua51("\"\\q\"");
    assert!(result.errors.is_empty(), "5.1 accepts \\q as literal 'q'");
}

#[test]
fn invalid_multibyte_escape_recovers_on_char_boundary() {
    // The invalid-escape error path must consume the whole escaped char.
    // Stopping mid-sequence leaves the cursor off a char boundary.
    let result = lex("\"\\\u{7C5}\" y", LuaVersion::Lua55);
    assert!(!result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.errors[0].message.contains("invalid escape"));
    assert!(
        result.errors[0].message.contains('\u{7C5}'),
        "message should carry the full char, not its first byte: {:?}",
        result.errors[0].message
    );
}

#[test]
fn hex_escape_lexes_outside_lua51() {
    for version in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        let result = lex("\"\\x41\"", version);
        assert!(result.errors.is_empty(), "\\x41 should work in {version:?}");
    }
}

#[test]
fn hex_escape_with_a_non_hex_digit_is_error() {
    let result = lex("\"\\xG1\"", LuaVersion::Lua52);
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("hex digit"));
}

#[test]
fn whitespace_escape_lexes_outside_lua51() {
    for version in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        let result = lex("\"\\z \"", version);
        assert!(result.errors.is_empty(), "\\z should work in {version:?}");
    }
}

#[test]
fn unicode_escape_only_53_plus_and_luau() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        let result = lex("\"\\u{41}\"", version);
        assert!(result.errors.is_empty(), "\\u{{41}} should work in {version:?}");
    }
    // 5.2 is strict and lacks \u, so it errors. 5.1 laxly accepts any escaped
    // non-digit as that literal character, so it lexes (value "u{41}").
    let strict_result = lex("\"\\u{41}\"", LuaVersion::Lua52);
    assert!(!strict_result.errors.is_empty(), "\\u{{41}} should fail in Lua52");
    let lax_result = lex("\"\\u{41}\"", LuaVersion::Lua51);
    assert!(lax_result.errors.is_empty(), "\\u{{41}} lexes under 5.1 as literal chars");
}

#[test]
fn unicode_escape_without_digits_is_error() {
    let result = lex("\"\\u{}\"", LuaVersion::Lua53);
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("at least one hex digit"));
}

#[test]
fn unicode_escape_without_open_brace_is_error() {
    let result = lex("\"\\u41}\"", LuaVersion::Lua53);
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("`{`"));
}

#[test]
fn unicode_escape_codepoint_bounds() {
    let result = lex(r#""\u{7FFFFFFF}""#, LuaVersion::Lua54);
    assert_eq!(result.errors, Vec::<SourceError>::new());
    assert_eq!(result.tokens[0].kind, TokenKind::StringLiteral(r#""\u{7FFFFFFF}""#.into()));

    for overflow in [r#""\u{80000000}""#, r#""\u{FFFFFFFF}""#] {
        let overflowed = lex(overflow, LuaVersion::Lua54);
        assert!(!overflowed.errors.is_empty(), "{overflow} should overflow");
        assert!(overflowed.errors[0].message.contains("codepoint too large"));
    }
}

#[test]
fn long_string_levels() {
    for source in ["[[text]]", "[=[text]=]", "[==[text]==]", "[===[text]===]"] {
        assert_eq!(
            first_kind(source),
            TokenKind::StringLiteral(source.into()),
            "failed for: {source:?}"
        );
    }
}

#[test]
fn long_string_with_newlines() {
    let source = "[[line1\nline2]]";
    assert_eq!(first_kind(source), TokenKind::StringLiteral(source.into()));
}

#[test]
fn unterminated_long_string_is_error() {
    let result = lex_lua51("[[oops");
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("unterminated"));
}
