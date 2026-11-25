use luck_lexer::lex;
use luck_token::{Span, TokenKind};

use crate::common::{first_kind, kinds, tokens};

#[test]
fn keywords_lex_as_keyword_tokens() {
    let cases = [
        ("and", TokenKind::And),
        ("break", TokenKind::Break),
        ("do", TokenKind::Do),
        ("else", TokenKind::Else),
        ("elseif", TokenKind::ElseIf),
        ("end", TokenKind::End),
        ("false", TokenKind::False),
        ("for", TokenKind::For),
        ("function", TokenKind::Function),
        ("if", TokenKind::If),
        ("in", TokenKind::In),
        ("local", TokenKind::Local),
        ("nil", TokenKind::Nil),
        ("not", TokenKind::Not),
        ("or", TokenKind::Or),
        ("repeat", TokenKind::Repeat),
        ("return", TokenKind::Return),
        ("then", TokenKind::Then),
        ("true", TokenKind::True),
        ("until", TokenKind::Until),
        ("while", TokenKind::While),
    ];
    for (source, expected) in cases {
        assert_eq!(first_kind(source), expected, "failed for keyword: {source}");
    }
}

#[test]
fn identifiers_take_letters_digits_and_underscores() {
    for source in ["foo", "_bar", "_123", "abc_def", "x9_Y"] {
        assert_eq!(tokens(source), [(TokenKind::Identifier, source)]);
    }
}

#[test]
fn a_name_ends_at_the_first_byte_that_cannot_continue_it() {
    assert_eq!(
        tokens("abc.d9(e_f)g"),
        [
            (TokenKind::Identifier, "abc"),
            (TokenKind::Dot, "."),
            (TokenKind::Identifier, "d9"),
            (TokenKind::LeftParen, "("),
            (TokenKind::Identifier, "e_f"),
            (TokenKind::RightParen, ")"),
            (TokenKind::Identifier, "g"),
        ]
    );
}

#[test]
fn a_keyword_spelling_inside_a_longer_name_is_a_name() {
    for source in ["ends", "_end", "end_", "End", "iff", "forward", "functions", "localx", "nil2"] {
        assert_eq!(first_kind(source), TokenKind::Identifier, "{source:?}");
    }
}

#[test]
fn single_char_symbols_lex_to_their_kinds() {
    let cases = [
        ("+", TokenKind::Plus),
        ("-", TokenKind::Minus),
        ("*", TokenKind::Star),
        ("/", TokenKind::Slash),
        ("%", TokenKind::Percent),
        ("^", TokenKind::Caret),
        ("#", TokenKind::Hash),
        ("(", TokenKind::LeftParen),
        (")", TokenKind::RightParen),
        ("{", TokenKind::LeftBrace),
        ("}", TokenKind::RightBrace),
        ("[", TokenKind::LeftBracket),
        ("]", TokenKind::RightBracket),
        (";", TokenKind::Semicolon),
        (":", TokenKind::Colon),
        (",", TokenKind::Comma),
        (".x", TokenKind::Dot),
        ("=", TokenKind::Equal),
        ("<", TokenKind::Less),
        (">", TokenKind::Greater),
        ("&", TokenKind::Ampersand),
        ("|", TokenKind::Pipe),
        ("?", TokenKind::Question),
        ("@", TokenKind::At),
    ];
    for (source, expected) in cases {
        assert_eq!(first_kind(source), expected, "failed for: {source:?}");
    }
}

#[test]
fn multi_char_symbols_lex_to_their_kinds() {
    let cases = [
        ("==", TokenKind::EqualEqual),
        ("~=", TokenKind::TildeEqual),
        ("<=", TokenKind::LessEqual),
        (">=", TokenKind::GreaterEqual),
        ("..x", TokenKind::DotDot),
        ("...", TokenKind::DotDotDot),
        ("::", TokenKind::DoubleColon),
        ("//", TokenKind::FloorDiv),
        ("->", TokenKind::Arrow),
    ];
    for (source, expected) in cases {
        assert_eq!(first_kind(source), expected, "failed for: {source:?}");
    }
}

#[test]
fn whitespace_yields_no_tokens() {
    assert_eq!(
        tokens(" \t x \x0B\x0C + \r\n y  "),
        [(TokenKind::Identifier, "x"), (TokenKind::Plus, "+"), (TokenKind::Identifier, "y")]
    );
}

#[test]
fn each_token_records_whether_a_line_break_precedes_it() {
    let source = "\
a b
c --[[
]] d -- e
f --[[ g ]] h";
    let result = lex(source.as_bytes());
    let breaks: Vec<(&str, bool)> = result
        .tokens
        .iter()
        .map(|token| {
            (str::from_utf8(token.text(source.as_bytes())).unwrap(), token.has_line_break_before)
        })
        .collect();
    assert_eq!(
        breaks,
        [
            ("a", true),
            ("b", false),
            ("c", true),
            ("d", true),
            ("f", true),
            ("h", false),
            ("", false)
        ]
    );
}

#[test]
fn only_an_lf_breaks_a_line() {
    assert!(!lex(b"a\rb").tokens[1].has_line_break_before);
    assert!(lex(b"a\r\nb").tokens[1].has_line_break_before);
    assert!(lex(b"a\nb").tokens[1].has_line_break_before);
}

#[test]
fn token_stream_ends_with_eof() {
    let result = lex(b"x");
    assert_eq!(result.tokens.last().expect("tokens should not be empty").kind, TokenKind::Eof);
}

#[test]
fn empty_source_lexes_to_only_eof() {
    let result = lex(b"");
    assert_eq!(result.tokens.len(), 1);
    assert_eq!(result.tokens[0].kind, TokenKind::Eof);
}

/// Every scan can meet the end of the source, so each prefix ends one at a
/// different byte.
#[test]
fn every_prefix_of_a_source_lexes_to_an_eof_at_its_end() {
    let source = "\u{FEFF}#!luau
local name_longer_than_a_word, x = 0x1F.8p-3 + 1e+10 .. 0b1_01i, 1_000i
s = \"esc \\n \\z \t \\x41 \\u{1F600} \\065 \\q \u{e9}\" .. 'q\\\r
' t = [==[ long ]] ]=] ]==] --[[ block
comment ]] -- line \u{1F600}\r
`a {b .. `c {d}`} \\{ {{ e` f(...) ..= //= :: -> \x0B\x0C caf\u{e9}
";
    // `get` refuses a cut inside a char.
    for prefix in (0..=source.len()).filter_map(|length| source.get(..length)) {
        let result = lex(prefix.as_bytes());
        let end = u32::try_from(prefix.len()).expect("the source is short");
        let (eof, tokens) = result.tokens.split_last().expect("an EOF token");
        assert_eq!(eof.kind, TokenKind::Eof, "{prefix:?}");
        assert_eq!(eof.span, Span::new(end, end), "{prefix:?}");
        let mut previous_end = 0;
        for token in tokens {
            let span = token.span;
            assert!(
                previous_end <= span.start && span.start < span.end && span.end <= end,
                "{prefix:?} {token:?}"
            );
            previous_end = span.end;
        }
        for comment in &result.comments {
            assert!(comment.span.end <= end, "{prefix:?} {comment:?}");
        }
    }
}

#[test]
fn token_spans_cover_their_source_bytes() {
    let result = lex(b"local x = 42");
    assert_eq!(result.tokens[0].span, Span::new(0, 5));
    assert_eq!(result.tokens[0].kind, TokenKind::Local);
    assert_eq!(result.tokens[1].span, Span::new(6, 7));
    assert_eq!(result.tokens[2].span, Span::new(8, 9));
    assert_eq!(result.tokens[3].span, Span::new(10, 12));
}

#[test]
fn local_stmt_lexes_token_by_token() {
    assert_eq!(
        kinds("local x = 1 + 2"),
        vec![
            TokenKind::Local,
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Number,
            TokenKind::Plus,
            TokenKind::Number,
        ]
    );
}

#[test]
fn call_with_string_argument_lexes_token_by_token() {
    assert_eq!(
        kinds("print(\"hello\")"),
        vec![
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::StringLiteral,
            TokenKind::RightParen,
        ]
    );
}

#[test]
fn table_constructor_lexes_token_by_token() {
    assert_eq!(
        kinds("{1, 2, 3}"),
        vec![
            TokenKind::LeftBrace,
            TokenKind::Number,
            TokenKind::Comma,
            TokenKind::Number,
            TokenKind::Comma,
            TokenKind::Number,
            TokenKind::RightBrace,
        ]
    );
}

#[test]
fn field_and_method_access_lex_token_by_token() {
    assert_eq!(
        kinds("a.b:c()"),
        vec![
            TokenKind::Identifier,
            TokenKind::Dot,
            TokenKind::Identifier,
            TokenKind::Colon,
            TokenKind::Identifier,
            TokenKind::LeftParen,
            TokenKind::RightParen,
        ]
    );
}

#[test]
fn dot_runs_lex_as_varargs_concat_and_dot() {
    assert_eq!(kinds("... .. ."), vec![TokenKind::DotDotDot, TokenKind::DotDot, TokenKind::Dot]);
}

#[test]
fn non_ascii_unexpected_char_is_one_error() {
    let result = lex("local caf\u{e9} = 1".as_bytes());
    assert_eq!(result.errors.len(), 1, "errors: {:?}", result.errors);
    assert!(result.errors[0].message.contains('\u{e9}'));
}

#[test]
fn each_unexpected_byte_that_is_not_utf8_is_one_error() {
    let result = lex(b"local caf\xE9\x80 = 1");
    let errors: Vec<(&str, Option<Span>)> =
        result.errors.iter().map(|e| (&*e.message, e.span)).collect();
    assert_eq!(
        errors,
        [
            ("unexpected byte 0xE9", Some(Span::new(9, 10))),
            ("unexpected byte 0x80", Some(Span::new(10, 11)))
        ]
    );
    let kinds: Vec<TokenKind> = result.tokens.iter().map(|token| token.kind).collect();
    assert_eq!(
        kinds,
        [
            TokenKind::Local,
            TokenKind::Identifier,
            TokenKind::Equal,
            TokenKind::Number,
            TokenKind::Eof
        ]
    );
}

#[test]
fn goto_and_global_are_names() {
    for name in ["goto", "global"] {
        assert_eq!(tokens(name), [(TokenKind::Identifier, name)]);
    }
}

#[test]
fn a_contextual_keyword_lexes_as_its_own_kind() {
    for (text, kind) in [
        ("continue", TokenKind::Continue),
        ("export", TokenKind::Export),
        ("const", TokenKind::Const),
        ("type", TokenKind::Type),
        ("typeof", TokenKind::Typeof),
        ("read", TokenKind::Read),
        ("write", TokenKind::Write),
    ] {
        assert_eq!(first_kind(text), kind, "{text}");
    }
}

#[test]
fn floor_div_not_confused_with_comment() {
    assert_eq!(
        kinds(
            "\
-- comment
//"
        ),
        vec![TokenKind::FloorDiv]
    );
}

#[test]
fn a_lone_tilde_is_an_error() {
    let result = lex(b"a ~ b");
    let [error] = result.errors.as_slice() else {
        panic!("{:?}", result.errors);
    };
    assert_eq!(error.span, Some(Span::new(2, 3)));
    assert_eq!(
        kinds("a ~= b"),
        [TokenKind::Identifier, TokenKind::TildeEqual, TokenKind::Identifier]
    );
}

#[test]
fn doubled_angle_brackets_lex_as_two_tokens() {
    assert_eq!(kinds("<<"), [TokenKind::Less, TokenKind::Less]);
    assert_eq!(kinds(">>"), [TokenKind::Greater, TokenKind::Greater]);
    assert_eq!(kinds("<=>="), [TokenKind::LessEqual, TokenKind::GreaterEqual]);
}

#[test]
fn type_operators_lex_token_by_token() {
    assert_eq!(
        kinds("a & b | c?"),
        vec![
            TokenKind::Identifier,
            TokenKind::Ampersand,
            TokenKind::Identifier,
            TokenKind::Pipe,
            TokenKind::Identifier,
            TokenKind::Question,
        ]
    );
}

#[test]
fn compound_assignment_operators_lex_as_one_token() {
    for (source, expected) in [
        ("+=", TokenKind::PlusEqual),
        ("-=", TokenKind::MinusEqual),
        ("*=", TokenKind::StarEqual),
        ("/=", TokenKind::SlashEqual),
        ("//=", TokenKind::FloorDivEqual),
        ("%=", TokenKind::PercentEqual),
        ("^=", TokenKind::CaretEqual),
        ("..=", TokenKind::DotDotEqual),
    ] {
        assert_eq!(tokens(source), [(expected, source)], "{source}");
    }
}

#[test]
fn an_attribute_lexes_as_at_and_a_name() {
    assert_eq!(kinds("@native"), vec![TokenKind::At, TokenKind::Identifier]);
}

#[test]
fn a_nul_byte_ends_the_source() {
    let result = lex(b"return 1\0$");
    assert!(
        result.errors.is_empty(),
        "Luau reads a raw NUL as end of input, so the bytes after it should not be lexed: {:?}",
        result.errors
    );
    assert_eq!(result.tokens.len(), 3, "{:?}", result.tokens);
}

#[test]
fn a_bom_is_an_error() {
    let result = lex("\u{FEFF}return 1".as_bytes());
    assert!(
        !result.errors.is_empty(),
        "a leading BOM fails to compile under Luau, so it should be reported: {:?}",
        result.tokens
    );
}
