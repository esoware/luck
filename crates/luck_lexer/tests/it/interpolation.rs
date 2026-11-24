use luck_diagnostics::Diagnostic;
use luck_lexer::{Lexer, lex};
use luck_token::{Comment, LuaVersion, TokenKind};

use crate::common::{token_texts, tokens_in};

const BEGIN: TokenKind = TokenKind::InterpolatedStringBegin;
const MIDDLE: TokenKind = TokenKind::InterpolatedStringMiddle;
const END: TokenKind = TokenKind::InterpolatedStringEnd;
const SIMPLE: TokenKind = TokenKind::InterpolatedStringSimple;

#[test]
fn interpolated_string_without_exprs_lexes_as_one_simple_token() {
    for (source, text) in [("`hello`", "hello"), ("``", "")] {
        let result = lex(source.as_bytes(), LuaVersion::Luau);
        assert_eq!(result.errors, Vec::<Diagnostic>::new(), "{source:?}");
        let [simple, eof] = result.tokens.as_slice() else {
            panic!("{source:?}: {:?}", result.tokens);
        };
        assert_eq!(
            (simple.kind, str::from_utf8(simple.text(source.as_bytes())).unwrap()),
            (SIMPLE, text)
        );
        assert_eq!(str::from_utf8(simple.span.text(source.as_bytes())).unwrap(), source);
        assert!(!simple.is_unterminated);
        assert_eq!(eof.kind, TokenKind::Eof);
    }
}

#[test]
fn each_piece_spans_its_delimiters_and_carries_the_text_between_them() {
    let source = "`a{x}b{y}c`";
    let result = lex(source.as_bytes(), LuaVersion::Luau);
    assert_eq!(result.errors, Vec::<Diagnostic>::new());
    let spans: Vec<&str> = result
        .tokens
        .iter()
        .map(|token| str::from_utf8(token.span.text(source.as_bytes())).unwrap())
        .collect();
    assert_eq!(spans, ["`a{", "x", "}b{", "y", "}c`", ""]);
    assert_eq!(
        tokens_in(source, LuaVersion::Luau),
        [
            (BEGIN, "a"),
            (TokenKind::Identifier, "x"),
            (MIDDLE, "b"),
            (TokenKind::Identifier, "y"),
            (END, "c"),
        ]
    );
}

/// The parser reports a `{}` that holds no expression.
#[test]
fn an_empty_hole_lexes_as_the_pieces_around_it() {
    for (source, expected) in [
        ("`a{}b`", vec![(BEGIN, "a"), (END, "b")]),
        (
            "`{x}a{ --[[c]] }b`",
            vec![(BEGIN, ""), (TokenKind::Identifier, "x"), (MIDDLE, "a"), (END, "b")],
        ),
    ] {
        assert_eq!(tokens_in(source, LuaVersion::Luau), expected, "{source}");
    }
}

#[test]
fn interpolated_string_begin_stops_at_the_open_brace() {
    assert_eq!(tokens_in("`hello {", LuaVersion::Luau), [(BEGIN, "hello ")]);
}

#[test]
fn interpolated_index_expr_lexes_token_by_token() {
    assert_eq!(
        tokens_in("`{t[1]}`", LuaVersion::Luau),
        [
            (BEGIN, ""),
            (TokenKind::Identifier, "t"),
            (TokenKind::LeftBracket, "["),
            (TokenKind::Number, "1"),
            (TokenKind::RightBracket, "]"),
            (END, ""),
        ]
    );
}

#[test]
fn braces_inside_a_hole_nest_before_the_one_that_resumes_the_string() {
    assert_eq!(
        tokens_in("`{ {a} }b{ {} }`", LuaVersion::Luau),
        [
            (BEGIN, ""),
            (TokenKind::LeftBrace, "{"),
            (TokenKind::Identifier, "a"),
            (TokenKind::RightBrace, "}"),
            (MIDDLE, "b"),
            (TokenKind::LeftBrace, "{"),
            (TokenKind::RightBrace, "}"),
            (END, ""),
        ]
    );
}

#[test]
fn a_string_inside_a_hole_opens_its_own_pieces() {
    assert_eq!(
        tokens_in("`a{`b{c}d`}e`", LuaVersion::Luau),
        [(BEGIN, "a"), (BEGIN, "b"), (TokenKind::Identifier, "c"), (END, "d"), (END, "e"),]
    );
}

#[test]
fn unterminated_interpolated_string_is_error() {
    let result = lex(b"`hello", LuaVersion::Luau);
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn interpolated_string_double_brace_is_error() {
    let result = lex(b"`{{x}}`", LuaVersion::Luau);
    assert_ne!(result.errors, Vec::<Diagnostic>::new());
    assert!(result.errors[0].message.contains("`{{` is not allowed"));
}

/// The parser and the skip past its depth cap find where a string ends by its
/// `InterpolatedStringEnd` or `InterpolatedStringSimple`, so one still comes
/// when the string's text is malformed.
#[test]
fn a_string_still_ends_when_its_text_is_malformed() {
    let name = |text| (TokenKind::Identifier, text);
    let cases = [
        (
            "\
`{x}abc
y",
            vec![(BEGIN, ""), name("x"), (END, "abc"), name("y")],
            ("unterminated interpolated string", "}abc"),
        ),
        (
            "`{x}a{{y}}b`",
            vec![(BEGIN, ""), name("x"), (END, "a{{y}}b")],
            ("`{{` is not allowed in interpolated strings, use `\\{` for a literal brace", "{{"),
        ),
        (
            "`{x}a\\xZZ{y}b`",
            vec![(BEGIN, ""), name("x"), (MIDDLE, "a\\xZZ"), name("y"), (END, "b")],
            ("`\\x` escape requires exactly 2 hex digits, got 0", "\\x"),
        ),
        (
            "`a\\u{}{x}`",
            vec![(BEGIN, "a\\u{}"), name("x"), (END, "")],
            ("`\\u{}` requires at least one hex digit", "\\u{"),
        ),
        (
            "\
`abc
y",
            vec![(SIMPLE, "abc"), name("y")],
            ("unterminated interpolated string", "`abc"),
        ),
    ];
    for (source, expected, (message, text)) in cases {
        let result = lex(source.as_bytes(), LuaVersion::Luau);
        assert_eq!(token_texts(&result, source), expected, "{source:?}");
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
fn an_unterminated_string_flags_the_piece_it_cut_off() {
    for (source, cut_off) in [
        (
            "\
`{x}abc
y",
            vec!["}abc"],
        ),
        (
            "\
`abc
y",
            vec!["`abc"],
        ),
        ("`a\\`", vec!["`a\\`"]),
        ("`{x}a{y}b`", vec![]),
    ] {
        let result = lex(source.as_bytes(), LuaVersion::Luau);
        let flagged: Vec<&str> = result
            .tokens
            .iter()
            .filter(|token| token.is_unterminated)
            .map(|token| str::from_utf8(token.span.text(source.as_bytes())).unwrap())
            .collect();
        assert_eq!(flagged, cut_off, "{source:?}");
    }
}

/// The escaped backtick is text, so it stays when the string has no closer.
#[test]
fn an_unterminated_piece_keeps_all_of_its_text() {
    for (source, expected) in [
        ("`a\\`", vec![(SIMPLE, "a\\`")]),
        ("`{x}b\\`", vec![(BEGIN, ""), (TokenKind::Identifier, "x"), (END, "b\\`")]),
    ] {
        let result = lex(source.as_bytes(), LuaVersion::Luau);
        assert_eq!(token_texts(&result, source), expected, "{source:?}");
    }
}

#[test]
fn backtick_is_an_unexpected_character_outside_luau() {
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        for source in ["`hello`", "`a \\\r\n{n}`"] {
            let result = lex(source.as_bytes(), version);
            assert!(
                result.errors.iter().any(|e| e.message == "unexpected character ```"),
                "{version:?} {source:?}: {:?}",
                result.errors
            );
        }
    }
}

#[test]
fn interpolated_string_keeps_raw_escapes() {
    assert_eq!(tokens_in("`hello\\nworld`", LuaVersion::Luau), [(SIMPLE, "hello\\nworld")]);
}

#[test]
fn interpolated_string_preserves_utf8() {
    assert_eq!(
        tokens_in("`h\u{e9}llo {x} w\u{f6}rld`", LuaVersion::Luau),
        [(BEGIN, "h\u{e9}llo "), (TokenKind::Identifier, "x"), (END, " w\u{f6}rld")]
    );
}

#[test]
fn interpolated_string_unicode_escape_is_not_interpolation() {
    // `\u{41}` is a unicode escape; the `{` must not open an expression.
    assert_eq!(tokens_in(r"`a\u{41}b`", LuaVersion::Luau), [(SIMPLE, r"a\u{41}b")]);
}

#[test]
fn interpolated_segments_preserve_raw_escapes_and_spans() {
    for count in [1, 64] {
        let text = "\u{e9}\\n\\u{41}\\{\\`\\\\\\z \r\n \u{6f22}".repeat(count);
        let source = format!("`{text}{{x}}{text}{{y}}{text}`");
        let result = lex(source.as_bytes(), LuaVersion::Luau);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        for (i, kind, spelling) in [
            (0, BEGIN, format!("`{text}{{")),
            (2, MIDDLE, format!("}}{text}{{")),
            (4, END, format!("}}{text}`")),
        ] {
            let token = result.tokens[i];
            assert_eq!(
                (token.kind, str::from_utf8(token.text(source.as_bytes())).unwrap()),
                (kind, text.as_str())
            );
            assert_eq!(token.span.text(source.as_bytes()), spelling.as_bytes());
        }
        let mut lexer =
            Lexer::new(source.as_bytes(), LuaVersion::Luau).expect("source fits u32 spans");
        for token in &result.tokens {
            assert_eq!(&lexer.next_token(), token);
        }
        let finished = lexer.finish();
        assert_eq!(finished.comments, Vec::<Comment>::new());
        assert!(finished.errors.is_empty(), "{:?}", finished.errors);
    }
}

#[test]
fn escaped_line_breaks_continue_interpolated_strings() {
    for newline in ["\r\n", "\n", "\r"] {
        let cases = [
            (
                format!("`a \\{newline}{{n}}`"),
                vec![
                    (BEGIN, format!("a \\{newline}")),
                    (TokenKind::Identifier, "n".to_owned()),
                    (END, String::new()),
                ],
            ),
            (format!("`a \\{newline}b`"), vec![(SIMPLE, format!("a \\{newline}b"))]),
            (
                format!("`{{n}}a \\{newline}{{m}}b \\{newline}`"),
                vec![
                    (BEGIN, String::new()),
                    (TokenKind::Identifier, "n".to_owned()),
                    (MIDDLE, format!("a \\{newline}")),
                    (TokenKind::Identifier, "m".to_owned()),
                    (END, format!("b \\{newline}")),
                ],
            ),
        ];
        for (source, owned) in cases {
            let expected: Vec<(TokenKind, &str)> =
                owned.iter().map(|(kind, text)| (*kind, text.as_str())).collect();
            assert_eq!(tokens_in(&source, LuaVersion::Luau), expected, "{source:?}");
        }
    }
}

#[test]
fn interpolated_string_rejects_unescaped_line_breaks() {
    for line_break in ["\n", "\r", "\r\n", "\\\n\r", "\\\r\n\n", "\\\r\n\r", "\\\\\r\n"] {
        for source in [
            format!("`a{line_break}b`"),
            format!("`a{line_break}{{n}}`"),
            format!("`{{n}}a{line_break}{{m}}`"),
            format!("`{{n}}a{line_break}`"),
        ] {
            let result = lex(source.as_bytes(), LuaVersion::Luau);
            assert!(
                result.errors.iter().any(|e| e.message == "unterminated interpolated string"),
                "{source:?}: {:?}",
                result.errors
            );
        }
    }
}

#[test]
fn interpolated_string_validates_escapes_like_short_strings() {
    for source in [r"`\u{}`", r"`\u41`", r"`\u{41`", r"`\999`", r"`\xZZ`", r"`{n} \256`"] {
        let result = lex(source.as_bytes(), LuaVersion::Luau);
        assert!(!result.errors.is_empty(), "{source:?} must be rejected");
    }
    for source in [
        r"`\{`",
        r"`\}`",
        r"`\``",
        r"`\u{41}`",
        r"`\x41\65\255\a\z  x`",
        r"`\q`",
        "`\\\n`",
        "`\\\r\n`",
        r"`{n} \n {m}`",
    ] {
        let result = lex(source.as_bytes(), LuaVersion::Luau);
        assert!(result.errors.is_empty(), "{source:?}: {:?}", result.errors);
    }
}
