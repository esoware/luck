use luck_diagnostics::Diagnostic;
use luck_lexer::lex;
use luck_token::{LuaVersion, Span, TokenKind};

use crate::common::{lex_lua51, tokens, tokens_in};

#[test]
fn quoted_strings_keep_their_spelling() {
    for source in ["\"hello\"", "'hello'"] {
        assert_eq!(tokens(source), [(TokenKind::StringLiteral, source)]);
    }
}

#[test]
fn a_string_says_whether_it_holds_what_its_value_rewrites() {
    let source = "x = 'a' .. \"b\\n\" .. [[c]] .. [[d\r\ne]] .. `f` .. `g\\{` .. 'h' f(i)";
    let result = lex(source.as_bytes(), LuaVersion::Luau);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let flags: Vec<(&str, bool)> = result
        .tokens
        .iter()
        .filter(|token| {
            matches!(token.kind, TokenKind::StringLiteral | TokenKind::InterpolatedStringSimple)
        })
        .map(|token| {
            (str::from_utf8(token.text(source.as_bytes())).unwrap_or_default(), token.has_escape)
        })
        .collect();
    assert_eq!(
        flags,
        [
            ("'a'", false),
            ("\"b\\n\"", true),
            ("[[c]]", false),
            ("[[d\r\ne]]", true),
            ("f", false),
            ("g\\{", true),
            ("'h'", false),
        ]
    );
}

#[test]
fn single_char_escapes_keep_their_spelling() {
    for source in [
        "\"\\a\"", "\"\\b\"", "\"\\f\"", "\"\\n\"", "\"\\r\"", "\"\\t\"", "\"\\v\"", "\"\\\\\"",
        "\"\\\"\"", "'\\'x'",
    ] {
        assert_eq!(tokens(source), [(TokenKind::StringLiteral, source)], "{source:?}");
    }
}

#[test]
fn decimal_escapes_up_to_255_lex() {
    // \065 = 'A'
    for source in ["\"\\065\"", "\"\\255\""] {
        assert_eq!(tokens(source), [(TokenKind::StringLiteral, source)]);
    }
}

#[test]
fn decimal_escape_over_255_is_error() {
    let result = lex_lua51("\"\\256\"");
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("too large"));
}

#[test]
fn escaped_line_breaks_continue_short_strings_in_every_version() {
    for version in LuaVersion::ALL {
        for quote in ['\'', '"'] {
            for newline in ["\r\n", "\n", "\r"] {
                let source = format!("{quote}a \\{newline}b{quote}");
                assert_eq!(
                    tokens_in(&source, version),
                    [(TokenKind::StringLiteral, source.as_str())],
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
                assert_eq!(
                    tokens_in(&source, version),
                    [(TokenKind::StringLiteral, source.as_str())]
                );
                for newline in ["\n", "\r", "\r\n"] {
                    let broken_source = format!("{quote}{text}{newline}{text}{quote}");
                    let broken_result = lex(broken_source.as_bytes(), version);
                    assert!(
                        broken_result.errors.iter().any(|e| e.message == "unterminated string"),
                        "{broken_source:?}: {:?}",
                        broken_result.errors
                    );
                }
            }
        }
    }
}

/// A string cut off at its line break, or holding a malformed escape, keeps
/// its token, so the rest of its line stays in it and the next line lexes as
/// it would have.
#[test]
fn a_malformed_string_keeps_its_token() {
    let name = |text| (TokenKind::Identifier, text);
    let string = |text| (TokenKind::StringLiteral, text);
    let cases = [
        (
            LuaVersion::Lua54,
            "\
\"abc
y",
            vec![string("\"abc"), name("y")],
            "unterminated string",
            "\"abc",
            true,
        ),
        (
            LuaVersion::Lua54,
            "'a\\qb' .. y",
            vec![string("'a\\qb'"), (TokenKind::DotDot, ".."), name("y")],
            "invalid escape sequence `\\q`",
            "\\q",
            false,
        ),
        (
            LuaVersion::Lua54,
            "\"a\\x\" y",
            vec![string("\"a\\x\""), name("y")],
            "`\\x` escape requires exactly 2 hex digits, got 0",
            "\\x",
            false,
        ),
        (LuaVersion::Lua54, "\"a\\", vec![string("\"a\\")], "unterminated string", "\"a\\", true),
        (
            LuaVersion::Lua51,
            "\
[[a
y",
            vec![string(
                "\
[[a
y",
            )],
            "unterminated long bracket string",
            "\
[[a
y",
            true,
        ),
        (
            LuaVersion::Lua51,
            "[[a [[b]] y",
            vec![string("[[a [[b]]"), name("y")],
            "nesting of `[[...]]` is deprecated",
            "[[",
            false,
        ),
    ];
    for (version, source, expected, message, text, is_unterminated) in cases {
        let result = lex(source.as_bytes(), version);
        let tokens: Vec<(TokenKind, &str)> = result
            .tokens
            .iter()
            .filter(|token| token.kind != TokenKind::Eof)
            .map(|token| (token.kind, str::from_utf8(token.text(source.as_bytes())).unwrap()))
            .collect();
        assert_eq!(tokens, expected, "{source:?}");
        assert_eq!(result.tokens[0].is_unterminated, is_unterminated, "{source:?}");
        let [error] = result.errors.as_slice() else {
            panic!("{source:?}: {:?}", result.errors);
        };
        assert_eq!(
            (&*error.message, error.span.map(|span| span.text(source.as_bytes()))),
            (message, Some(text.as_bytes())),
            "{source:?}"
        );
    }
}

#[test]
fn unterminated_string_is_error() {
    let result = lex_lua51("\"hello");
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn undefined_escape_is_error_on_lua52_and_later() {
    // 5.2+ rejects undefined escapes; 5.1 and Luau accept them as the
    // literal character.
    let strict_result = lex(b"\"\\q\"", LuaVersion::Lua52);
    assert!(!strict_result.errors.is_empty(), "Lua52 must reject \\q");
    assert!(strict_result.errors[0].message.contains("invalid escape"));
    let result = lex_lua51("\"\\q\"");
    assert!(result.errors.is_empty(), "5.1 accepts \\q as literal 'q'");
}

#[test]
fn invalid_multibyte_escape_recovers_on_char_boundary() {
    // The invalid-escape error path must consume the whole escaped char.
    // Stopping mid-sequence leaves the cursor off a char boundary.
    let result = lex("\"\\\u{7C5}\" y".as_bytes(), LuaVersion::Lua55);
    assert!(!result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.errors[0].message.contains("invalid escape"));
    assert!(
        result.errors[0].message.contains('\u{7C5}'),
        "message should carry the full char, not its first byte: {:?}",
        result.errors[0].message
    );
}

#[test]
fn a_string_holds_bytes_that_are_not_utf8_as_written() {
    for source in [&b"'caf\xE9'"[..], b"\"\xFF\xFE\"", b"[[\xE9\n\xE8]]", b"[==[\x80]==]"] {
        for version in LuaVersion::ALL {
            let result = lex(source, version);
            assert!(result.errors.is_empty(), "{source:?} {version:?}: {:?}", result.errors);
            let [string, eof] = result.tokens.as_slice() else {
                panic!("{source:?} {version:?}: {:?}", result.tokens);
            };
            assert_eq!((string.kind, string.text(source)), (TokenKind::StringLiteral, source));
            assert_eq!(eof.kind, TokenKind::Eof);
        }
    }
}

#[test]
fn an_escaped_byte_that_is_not_utf8_is_lenient_or_invalid_as_the_version_reads_it() {
    let source = b"'\\\xE9'";
    let lenient = lex(source, LuaVersion::Lua51);
    assert!(lenient.errors.is_empty(), "{:?}", lenient.errors);
    assert_eq!(lenient.lenient_escapes, [Span::new(1, 3)]);
    let strict = lex(source, LuaVersion::Lua54);
    let errors: Vec<(&str, Option<Span>)> =
        strict.errors.iter().map(|e| (&*e.message, e.span)).collect();
    assert_eq!(errors, [("invalid escape sequence, `\\` before byte 0xE9", Some(Span::new(1, 3)))]);
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
        let result = lex(b"\"\\x41\"", version);
        assert!(result.errors.is_empty(), "\\x41 should work in {version:?}");
    }
}

#[test]
fn hex_escape_with_a_non_hex_digit_is_error() {
    let result = lex(b"\"\\xG1\"", LuaVersion::Lua52);
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
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
        let result = lex(b"\"\\z \"", version);
        assert!(result.errors.is_empty(), "\\z should work in {version:?}");
    }
}

#[test]
fn unicode_escape_lexes_everywhere_but_lua52() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        let result = lex(b"\"\\u{41}\"", version);
        assert!(result.errors.is_empty(), "\\u{{41}} should work in {version:?}");
    }
    // 5.2 is strict and lacks \u, so it errors. 5.1 laxly accepts any escaped
    // non-digit as that literal character, so it lexes (value "u{41}").
    let strict_result = lex(b"\"\\u{41}\"", LuaVersion::Lua52);
    assert!(!strict_result.errors.is_empty(), "\\u{{41}} should fail in Lua52");
    let lax_result = lex(b"\"\\u{41}\"", LuaVersion::Lua51);
    assert!(lax_result.errors.is_empty(), "\\u{{41}} lexes under 5.1 as literal chars");
}

#[test]
fn unicode_escape_without_digits_is_error() {
    let result = lex(b"\"\\u{}\"", LuaVersion::Lua53);
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("at least one hex digit"));
}

#[test]
fn unicode_escape_without_open_brace_is_error() {
    let result = lex(b"\"\\u41}\"", LuaVersion::Lua53);
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("`{`"));
}

#[test]
fn unicode_escape_codepoint_must_be_below_2_pow_31() {
    let source = r#""\u{7FFFFFFF}""#;
    assert_eq!(tokens_in(source, LuaVersion::Lua54), [(TokenKind::StringLiteral, source)]);

    for overflow in [r#""\u{80000000}""#, r#""\u{FFFFFFFF}""#] {
        let overflowed = lex(overflow.as_bytes(), LuaVersion::Lua54);
        assert!(!overflowed.errors.is_empty(), "{overflow} should overflow");
        assert!(overflowed.errors[0].message.contains("codepoint too large"));
    }
}

#[test]
fn long_strings_keep_their_spelling_at_every_level() {
    for source in ["[[text]]", "[=[text]=]", "[==[text]==]", "[===[text]===]"] {
        assert_eq!(tokens(source), [(TokenKind::StringLiteral, source)], "{source:?}");
    }
}

#[test]
fn long_string_keeps_its_line_breaks() {
    let source = "\
[[line1
line2]]";
    assert_eq!(tokens(source), [(TokenKind::StringLiteral, source)]);
}

#[test]
fn unterminated_long_string_is_error() {
    let result = lex_lua51("[[oops");
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn luau_undefined_escape_lexes_as_the_literal_character() {
    let result = lex(br#""\q""#, LuaVersion::Luau);
    assert_eq!(
        result.errors,
        Vec::<Diagnostic>::new(),
        "Luau reads an undefined escape as its literal character, so it should lex without errors"
    );
}

#[test]
fn records_where_each_lenient_escape_is() {
    let source = "x = '\\q\\n\\\u{e9}' .. `\\w\\{{a}\\}`";
    let result = lex(source.as_bytes(), LuaVersion::Luau);
    assert_eq!(result.errors, Vec::<Diagnostic>::new());
    let escapes: Vec<&str> = result
        .lenient_escapes
        .iter()
        .map(|span| str::from_utf8(span.text(source.as_bytes())).unwrap())
        .collect();
    assert_eq!(escapes, ["\\q", "\\\u{e9}", "\\w"]);
    let lua51 = lex(b"x = '\\x41\\z\\u{48}\\65'", LuaVersion::Lua51);
    assert_eq!(lua51.lenient_escapes.len(), 3, "{:?}", lua51.lenient_escapes);
    for version in [LuaVersion::Lua52, LuaVersion::Lua54] {
        assert_eq!(lex(b"x = '\\x41\\z'", version).lenient_escapes, Vec::<Span>::new());
    }
}

#[test]
fn unicode_escape_above_10ffff_is_error_on_lua53_and_luau() {
    for version in [LuaVersion::Lua53, LuaVersion::Luau] {
        let result = lex(br#""\u{110000}""#, version);
        assert!(
            !result.errors.is_empty(),
            "a codepoint above U+10FFFF should be too large under {version:?}: {:?}",
            result.tokens
        );
        assert!(result.errors[0].message.contains("0x110000"), "{:?}", result.errors);
    }
}

/// The error is at the first nested opener, where Lua 5.1's
/// `read_long_string` raises it, not over the whole string or comment.
#[test]
fn lua51_level_zero_long_bracket_rejects_a_nested_opener_where_it_is() {
    for (source, nested_open) in [
        ("return [[ a [[ b ]]", 12),
        ("return [[ a [[ b [[ c ]]", 12),
        ("return [[[[]]", 9),
        (
            "\
--[[ a [[ b ]]
return 1",
            7,
        ),
    ] {
        let result = lex(source.as_bytes(), LuaVersion::Lua51);
        let [error] = result.errors.as_slice() else {
            panic!("{source:?}: {:?}", result.errors);
        };
        assert_eq!(error.message, "nesting of `[[...]]` is deprecated", "{source:?}");
        assert_eq!(error.span, Some(Span::new(nested_open, nested_open + 2)), "{source:?}");
    }
}

#[test]
fn luau_raw_nul_in_a_short_string_is_unterminated() {
    let result = lex(b"return 'a\0b'", LuaVersion::Luau);
    assert!(
        !result.errors.is_empty(),
        "a raw NUL ends the source in Luau, so the string should be unterminated: {:?}",
        result.tokens
    );
}
