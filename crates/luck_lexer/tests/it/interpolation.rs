use luck_lexer::{Lexer, lex};
use luck_token::{Comment, CompactString, LuaVersion, SourceError, TokenKind};

use crate::common::kinds_in;

#[test]
fn interpolated_string_without_exprs_lexes_as_a_begin_end_pair() {
    assert_eq!(
        kinds_in("`hello`", LuaVersion::Luau),
        vec![
            TokenKind::InterpolatedStringBegin(CompactString::default()),
            TokenKind::InterpolatedStringEnd("hello".into()),
        ]
    );
}

#[test]
fn empty_interpolated_string_lexes_as_an_empty_begin_end_pair() {
    assert_eq!(
        kinds_in("``", LuaVersion::Luau),
        vec![
            TokenKind::InterpolatedStringBegin(CompactString::default()),
            TokenKind::InterpolatedStringEnd(CompactString::default()),
        ]
    );
}

#[test]
fn interpolated_string_begin_stops_at_the_open_brace() {
    let result = lex("`hello {", LuaVersion::Luau);
    assert_eq!(result.errors, Vec::<SourceError>::new());
    assert_eq!(result.tokens[0].kind, TokenKind::InterpolatedStringBegin("hello ".into()));
}

#[test]
fn interpolated_index_expr_lexes_token_by_token() {
    assert_eq!(
        kinds_in("`{t[1]}`", LuaVersion::Luau),
        vec![
            TokenKind::InterpolatedStringBegin(CompactString::default()),
            TokenKind::Identifier("t".into()),
            TokenKind::LeftBracket,
            TokenKind::Number("1".into()),
            TokenKind::RightBracket,
            TokenKind::InterpolatedStringEnd(CompactString::default()),
        ]
    );
}

#[test]
fn unterminated_interpolated_string_is_error() {
    let result = lex("`hello", LuaVersion::Luau);
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn interpolated_string_double_brace_is_error() {
    let result = lex("`{{x}}`", LuaVersion::Luau);
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("`{{` is not allowed"));
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
            let result = lex(source, version);
            assert!(
                result.errors.iter().any(|error| error.message == "unexpected character ```"),
                "{version:?} {source:?}: {:?}",
                result.errors
            );
        }
    }
}

#[test]
fn interpolated_string_keeps_raw_escapes() {
    assert_eq!(
        kinds_in("`hello\\nworld`", LuaVersion::Luau)[1],
        TokenKind::InterpolatedStringEnd("hello\\nworld".into())
    );
}

#[test]
fn interpolated_string_preserves_utf8() {
    let result = lex("`h\u{e9}llo {x} w\u{f6}rld`", LuaVersion::Luau);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    let TokenKind::InterpolatedStringBegin(begin_text) = &result.tokens[0].kind else {
        panic!("expected InterpolatedStringBegin, got {:?}", result.tokens[0].kind);
    };
    assert_eq!(begin_text.as_str(), "h\u{e9}llo ");
    let end = result
        .tokens
        .iter()
        .find_map(|token| {
            if let TokenKind::InterpolatedStringEnd(text) = &token.kind {
                Some(text.as_str().to_owned())
            } else {
                None
            }
        })
        .expect("InterpolatedStringEnd token");
    assert_eq!(end, " w\u{f6}rld");
}

#[test]
fn interpolated_string_unicode_escape_is_not_interpolation() {
    // `\u{41}` is a unicode escape; the `{` must not open an expression.
    let result = lex(r"`a\u{41}b`", LuaVersion::Luau);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    let TokenKind::InterpolatedStringBegin(begin_text) = &result.tokens[0].kind else {
        panic!("expected InterpolatedStringBegin, got {:?}", result.tokens[0].kind);
    };
    // Plain string form: InterpolatedStringBegin("") + InterpolatedStringEnd(full text).
    assert_eq!(begin_text.as_str(), "");
    let TokenKind::InterpolatedStringEnd(end_text) = &result.tokens[1].kind else {
        panic!("expected InterpolatedStringEnd, got {:?}", result.tokens[1].kind);
    };
    assert_eq!(end_text.as_str(), r"a\u{41}b");
}

#[test]
fn interpolated_segments_preserve_raw_escapes_and_spans() {
    for count in [1, 64] {
        let text = "\u{e9}\\n\\u{41}\\{\\`\\\\\\z \r\n \u{6f22}".repeat(count);
        let source = format!("`{text}{{x}}{text}{{y}}{text}`");
        let result = lex(&source, LuaVersion::Luau);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.tokens[0].kind, TokenKind::InterpolatedStringBegin(text.as_str().into()));
        assert_eq!(
            result.tokens[2].kind,
            TokenKind::InterpolatedStringMiddle(text.as_str().into())
        );
        assert_eq!(result.tokens[4].kind, TokenKind::InterpolatedStringEnd(text.as_str().into()));
        for (index, expected) in
            [(0, format!("`{text}{{")), (2, format!("}}{text}{{")), (4, format!("}}{text}`"))]
        {
            let span = result.tokens[index].span;
            assert_eq!(span.text(&source), expected);
        }
        let mut lexer = Lexer::new(&source, LuaVersion::Luau).expect("source fits u32 spans");
        for token in &result.tokens {
            assert_eq!(&lexer.next_token(), token);
        }
        let (comments, errors) = lexer.finish();
        assert_eq!(comments, Vec::<Comment>::new());
        assert!(errors.is_empty(), "{errors:?}");
    }
}

#[test]
fn escaped_line_breaks_continue_interpolated_strings() {
    for newline in ["\r\n", "\n", "\r"] {
        let cases = [
            (
                format!("`a \\{newline}{{n}}`"),
                vec![
                    TokenKind::InterpolatedStringBegin(format!("a \\{newline}").into()),
                    TokenKind::Identifier("n".into()),
                    TokenKind::InterpolatedStringEnd(CompactString::default()),
                ],
            ),
            (
                format!("`a \\{newline}b`"),
                vec![
                    TokenKind::InterpolatedStringBegin(CompactString::default()),
                    TokenKind::InterpolatedStringEnd(format!("a \\{newline}b").into()),
                ],
            ),
            (
                format!("`{{n}}a \\{newline}{{m}}b \\{newline}`"),
                vec![
                    TokenKind::InterpolatedStringBegin(CompactString::default()),
                    TokenKind::Identifier("n".into()),
                    TokenKind::InterpolatedStringMiddle(format!("a \\{newline}").into()),
                    TokenKind::Identifier("m".into()),
                    TokenKind::InterpolatedStringEnd(format!("b \\{newline}").into()),
                ],
            ),
        ];
        for (source, expected) in cases {
            assert_eq!(kinds_in(&source, LuaVersion::Luau), expected, "{source:?}");
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
            let result = lex(&source, LuaVersion::Luau);
            assert!(
                result
                    .errors
                    .iter()
                    .any(|error| error.message == "unterminated interpolated string"),
                "{source:?}: {:?}",
                result.errors
            );
        }
    }
}

#[test]
fn interpolated_string_validates_escapes_like_short_strings() {
    for source in [r"`\u{}`", r"`\u41`", r"`\u{41`", r"`\999`", r"`\xZZ`", r"`{n} \256`"] {
        let result = lex(source, LuaVersion::Luau);
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
        let result = lex(source, LuaVersion::Luau);
        assert!(result.errors.is_empty(), "{source:?}: {:?}", result.errors);
    }
}
