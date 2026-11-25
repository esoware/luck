use luck_diagnostics::Diagnostic;
use luck_lexer::lex;
use luck_token::{Span, TokenKind};

use crate::common::{token_texts, tokens};

#[test]
fn quoted_strings_keep_their_spelling() {
    for source in ["\"hello\"", "'hello'"] {
        assert_eq!(tokens(source), [(TokenKind::StringLiteral, source)]);
    }
}

#[test]
fn a_string_says_whether_it_holds_what_its_value_rewrites() {
    let source = "x = 'a' .. \"b\\n\" .. [[c]] .. [[d\r\ne]] .. `f` .. `g\\{` .. 'h' f(i)";
    let result = lex(source.as_bytes());
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let flags: Vec<(&str, bool)> = result
        .tokens
        .iter()
        .filter(|token| {
            matches!(token.kind, TokenKind::StringLiteral | TokenKind::InterpolatedStringSimple)
        })
        .map(|token| (str::from_utf8(token.text(source.as_bytes())).unwrap(), token.has_escape))
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
    let result = lex(b"\"\\256\"");
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("too large"));
}

#[test]
fn escaped_line_breaks_continue_short_strings() {
    for quote in ['\'', '"'] {
        for newline in ["\r\n", "\n", "\r"] {
            let source = format!("{quote}a \\{newline}b{quote}");
            let expected = [(TokenKind::StringLiteral, source.as_str())];
            assert_eq!(tokens(&source), expected, "{source:?}");
        }
    }
}

#[test]
fn short_string_scan_boundaries_preserve_spelling_and_errors() {
    for length in [0, 1, 7, 8, 9, 15, 16, 31, 32, 33, 127, 128, 1024] {
        for quote in ['\'', '"'] {
            let opposite = if quote == '\'' { '"' } else { '\'' };
            let text = format!("{}\u{e9}{opposite}\u{6f22}", "a".repeat(length));
            let source = format!("{quote}{text}\\n{text}{quote}");
            assert_eq!(tokens(&source), [(TokenKind::StringLiteral, source.as_str())]);
            for newline in ["\n", "\r", "\r\n"] {
                let broken_source = format!("{quote}{text}{newline}{text}{quote}");
                let broken_result = lex(broken_source.as_bytes());
                assert!(
                    broken_result.errors.iter().any(|e| e.message == "unterminated string"),
                    "{broken_source:?}: {:?}",
                    broken_result.errors
                );
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
            "\
\"abc
y",
            vec![string("\"abc"), name("y")],
            "unterminated string",
            "\"abc",
            true,
        ),
        (
            "'a\\u{110000}b' .. y",
            vec![string("'a\\u{110000}b'"), (TokenKind::DotDot, ".."), name("y")],
            "`\\u` codepoint too large (must be less than 0x110000)",
            "\\u{110000",
            false,
        ),
        (
            "\"a\\x\" y",
            vec![string("\"a\\x\""), name("y")],
            "`\\x` escape requires exactly 2 hex digits, got 0",
            "\\x",
            false,
        ),
        ("\"a\\", vec![string("\"a\\")], "unterminated string", "\"a\\", true),
        (
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
    ];
    for (source, expected, message, text, is_unterminated) in cases {
        let result = lex(source.as_bytes());
        assert_eq!(token_texts(&result, source), expected, "{source:?}");
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
    let result = lex(b"\"hello");
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn a_lenient_multibyte_escape_spans_its_whole_char() {
    // Stopping mid-sequence leaves the cursor off a char boundary.
    let source = "\"\\\u{7C5}\" y";
    let result = lex(source.as_bytes());
    assert_eq!(result.errors, Vec::<Diagnostic>::new());
    assert_eq!(result.lenient_escapes, [Span::from_indices(1, 4)]);
    assert_eq!(token_texts(&result, source)[1], (TokenKind::Identifier, "y"));
}

#[test]
fn a_string_holds_bytes_that_are_not_utf8_as_written() {
    for source in [&b"'caf\xE9'"[..], b"\"\xFF\xFE\"", b"[[\xE9\n\xE8]]", b"[==[\x80]==]"] {
        let result = lex(source);
        assert!(result.errors.is_empty(), "{source:?}: {:?}", result.errors);
        let [string, eof] = result.tokens.as_slice() else {
            panic!("{source:?}: {:?}", result.tokens);
        };
        assert_eq!((string.kind, string.text(source)), (TokenKind::StringLiteral, source));
        assert_eq!(eof.kind, TokenKind::Eof);
    }
}

#[test]
fn an_escaped_byte_that_is_not_utf8_is_lenient() {
    let result = lex(b"'\\\xE9'");
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lenient_escapes, [Span::new(1, 3)]);
}

#[test]
fn hex_escape_with_a_non_hex_digit_is_error() {
    let result = lex(b"\"\\xG1\"");
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("hex digit"));
}

#[test]
fn unicode_escape_without_digits_is_error() {
    let result = lex(b"\"\\u{}\"");
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("at least one hex digit"));
}

#[test]
fn unicode_escape_without_open_brace_is_error() {
    let result = lex(b"\"\\u41}\"");
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("`{`"));
}

#[test]
fn escapes_of_every_kind_lex_without_errors() {
    for source in [r#""\x41""#, r#""\z  ""#, r#""\u{41}""#, r#""\u{10FFFF}""#, r#""\q""#] {
        assert_eq!(tokens(source), [(TokenKind::StringLiteral, source)]);
    }
}

#[test]
fn unicode_escape_codepoint_must_be_below_0x110000() {
    for overflow in [r#""\u{110000}""#, r#""\u{7FFFFFFF}""#, r#""\u{FFFFFFFFF}""#] {
        let overflowed = lex(overflow.as_bytes());
        assert!(!overflowed.errors.is_empty(), "{overflow} should overflow");
        assert!(overflowed.errors[0].message.contains("0x110000"), "{:?}", overflowed.errors);
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
    let result = lex(b"[[oops");
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn records_where_each_lenient_escape_is() {
    let source = "x = '\\q\\n\\\u{e9}' .. `\\w\\{{a}\\}`";
    let result = lex(source.as_bytes());
    assert_eq!(result.errors, Vec::<Diagnostic>::new());
    let escapes: Vec<&str> = result
        .lenient_escapes
        .iter()
        .map(|span| str::from_utf8(span.text(source.as_bytes())).unwrap())
        .collect();
    assert_eq!(escapes, ["\\q", "\\\u{e9}", "\\w"]);
    assert_eq!(lex(b"x = '\\x41\\z\\u{48}\\65'").lenient_escapes, Vec::<Span>::new());
}

/// A long bracket holds everything up to its closer, another opener
/// included.
#[test]
fn a_long_bracket_holds_an_opener() {
    for source in ["return [[ a [[ b ]]", "return [[[[]]", "--[[ a [[ b ]]\nreturn 1"] {
        assert_eq!(lex(source.as_bytes()).errors, Vec::<Diagnostic>::new(), "{source:?}");
    }
}

#[test]
fn a_raw_nul_in_a_short_string_leaves_it_unterminated() {
    let result = lex(b"return 'a\0b'");
    assert!(
        !result.errors.is_empty(),
        "a raw NUL ends the source in Luau, so the string should be unterminated: {:?}",
        result.tokens
    );
}
