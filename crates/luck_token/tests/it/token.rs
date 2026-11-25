use luck_token::{Span, Token, TokenKind};

const fn token(kind: TokenKind, start: u32, end: u32) -> Token {
    Token {
        span: Span::new(start, end),
        kind,
        has_line_break_before: false,
        is_unterminated: false,
        has_escape: false,
    }
}

// The parser copies a token at every step and `lex` returns every token in
// one buffer, so a size regression costs time and memory on every file.
#[test]
fn token_size_is_pinned() {
    assert_eq!(size_of::<TokenKind>(), 1);
    assert_eq!(size_of::<Token>(), 12);
    assert_eq!(size_of::<Option<Token>>(), 12);
}

#[test]
fn display_names_each_kind_without_its_text() {
    for (kind, expected) in [
        (TokenKind::Identifier, "identifier"),
        (TokenKind::Number, "number"),
        (TokenKind::Integer, "integer"),
        (TokenKind::StringLiteral, "string"),
        (TokenKind::InterpolatedStringBegin, "interpolated string start"),
        (TokenKind::InterpolatedStringMiddle, "interpolated string middle"),
        (TokenKind::InterpolatedStringEnd, "interpolated string end"),
        (TokenKind::InterpolatedStringSimple, "interpolated string"),
        (TokenKind::Eof, "end of file"),
    ] {
        assert_eq!(kind.to_string(), expected);
    }
}

#[test]
fn display_quotes_symbols_and_keywords() {
    assert_eq!(TokenKind::And.to_string(), "`and`");
    assert_eq!(TokenKind::DotDotDot.to_string(), "`...`");
    assert_eq!(TokenKind::LeftBrace.to_string(), "`{`");
    assert_eq!(TokenKind::RightBrace.to_string(), "`}`");
    assert_eq!(TokenKind::Arrow.to_string(), "`->`");
}

#[test]
fn token_display_shows_name_and_numeral_text_but_not_string_text() {
    let source = "foo 42 1i \"secret\" }secret{ type";
    for (kind, start, end, expected) in [
        (TokenKind::Identifier, 0, 3, "identifier `foo`"),
        (TokenKind::Type, 28, 32, "identifier `type`"),
        (TokenKind::Number, 4, 6, "number `42`"),
        (TokenKind::Integer, 7, 9, "integer `1i`"),
        (TokenKind::StringLiteral, 10, 18, "string"),
        (TokenKind::InterpolatedStringMiddle, 19, 27, "interpolated string middle"),
        (TokenKind::Eof, 27, 27, "end of file"),
    ] {
        assert_eq!(token(kind, start, end).display(source.as_bytes()).to_string(), expected);
    }
}

#[test]
fn static_text_is_none_only_for_kinds_whose_text_varies_and_eof() {
    assert_eq!(TokenKind::Continue.static_text(), Some("continue"));
    assert_eq!(TokenKind::DoubleColon.static_text(), Some("::"));
    for kind in [
        TokenKind::Identifier,
        TokenKind::Number,
        TokenKind::Integer,
        TokenKind::StringLiteral,
        TokenKind::InterpolatedStringBegin,
        TokenKind::InterpolatedStringMiddle,
        TokenKind::InterpolatedStringEnd,
        TokenKind::InterpolatedStringSimple,
        TokenKind::Eof,
    ] {
        assert_eq!(kind.static_text(), None, "{kind:?}");
    }
}

#[test]
fn text_is_the_whole_spelling_of_a_name_numeral_string_or_symbol() {
    let source = "name 0x1F \"a\\n\" [[b]] ...";
    for (kind, start, end, expected) in [
        (TokenKind::Identifier, 0, 4, "name"),
        (TokenKind::Number, 5, 9, "0x1F"),
        (TokenKind::StringLiteral, 10, 15, "\"a\\n\""),
        (TokenKind::StringLiteral, 16, 21, "[[b]]"),
        (TokenKind::DotDotDot, 22, 25, "..."),
        (TokenKind::Eof, 25, 25, ""),
    ] {
        assert_eq!(
            token(kind, start, end).text(source.as_bytes()),
            expected.as_bytes(),
            "{kind:?}"
        );
    }
}

#[test]
fn text_of_an_interpolated_string_piece_leaves_out_its_delimiters() {
    let source = "`a{x}b\\`{y}c` `d`";
    for (kind, start, end, expected) in [
        (TokenKind::InterpolatedStringBegin, 0, 3, "a"),
        (TokenKind::InterpolatedStringMiddle, 4, 9, "b\\`"),
        (TokenKind::InterpolatedStringEnd, 10, 13, "c"),
        (TokenKind::InterpolatedStringSimple, 14, 17, "d"),
    ] {
        assert_eq!(
            token(kind, start, end).text(source.as_bytes()),
            expected.as_bytes(),
            "{kind:?}"
        );
    }
}

/// An escaped backtick can end an unterminated piece, so only the flag tells
/// whether its last byte is a delimiter.
#[test]
fn text_of_an_unterminated_piece_keeps_its_last_byte() {
    let source = "`a\\` }b\\` `";
    for (kind, start, end, expected) in [
        (TokenKind::InterpolatedStringSimple, 0, 4, "a\\`"),
        (TokenKind::InterpolatedStringEnd, 5, 9, "b\\`"),
        (TokenKind::InterpolatedStringSimple, 10, 11, ""),
    ] {
        let unterminated = Token { is_unterminated: true, ..token(kind, start, end) };
        assert_eq!(unterminated.text(source.as_bytes()), expected.as_bytes(), "{kind:?}");
    }
}

const KEYWORDS: [&str; 28] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local",
    "nil", "not", "or", "repeat", "return", "then", "true", "until", "while", "continue", "export",
    "const", "type", "typeof", "read", "write",
];

#[test]
fn keyword_reads_back_each_keyword_spelling() {
    let base_keywords = [
        TokenKind::And,
        TokenKind::Break,
        TokenKind::Do,
        TokenKind::Else,
        TokenKind::ElseIf,
        TokenKind::End,
        TokenKind::False,
        TokenKind::For,
        TokenKind::Function,
        TokenKind::If,
        TokenKind::In,
        TokenKind::Local,
        TokenKind::Nil,
        TokenKind::Not,
        TokenKind::Or,
        TokenKind::Repeat,
        TokenKind::Return,
        TokenKind::Then,
        TokenKind::True,
        TokenKind::Until,
        TokenKind::While,
    ];
    for kind in base_keywords {
        let text = kind.static_text().unwrap();
        assert_eq!(TokenKind::keyword(text), Some(kind));
        assert!(!kind.is_name(), "{kind:?}");
    }
    for name in ["", "x", "End", "functions", "types"] {
        assert_eq!(TokenKind::keyword(name), None, "{name:?}");
    }
}

#[test]
fn keyword_reads_each_contextual_keyword_as_a_name_kind() {
    for (text, kind) in [
        ("continue", TokenKind::Continue),
        ("export", TokenKind::Export),
        ("const", TokenKind::Const),
        ("type", TokenKind::Type),
        ("typeof", TokenKind::Typeof),
        ("read", TokenKind::Read),
        ("write", TokenKind::Write),
    ] {
        assert_eq!(kind.static_text(), Some(text));
        assert!(kind.is_name(), "{kind:?}");
        assert_eq!(TokenKind::keyword(text), Some(kind));
    }
}

/// A name that hashes to a keyword's slot is still a name, so a lookup has to
/// compare the whole spelling.
#[test]
fn keyword_rejects_every_near_spelling_of_a_keyword() {
    for keyword in KEYWORDS {
        let (_, tail) = keyword.split_at(1);
        let (head, _) = keyword.split_at(keyword.len() - 1);
        let mut near = vec![
            tail.to_owned(),
            head.to_owned(),
            format!("{keyword}_"),
            format!("_{keyword}"),
            keyword.to_uppercase(),
        ];
        for i in 0..keyword.len() {
            let mut bytes = keyword.as_bytes().to_vec();
            bytes[i] ^= 1;
            near.push(String::from_utf8(bytes).expect("ASCII stays UTF-8"));
        }
        for name in near.iter().filter(|name| !KEYWORDS.contains(&name.as_str())) {
            assert_eq!(TokenKind::keyword(name), None, "{name:?}");
        }
    }
}

/// Every keyword, and every name one edit or one byte of length from one,
/// beside bytes that would change the answer if a lookup read past the name.
#[test]
fn of_name_reads_each_name_as_keyword_does() {
    let mut names: Vec<String> =
        ["x", "_", "e", "functions", "elseiff", "typeofx", "x1234567"].map(String::from).to_vec();
    for keyword in KEYWORDS {
        names.push(keyword.to_owned());
        names.push(format!("{keyword}s"));
        names.push(keyword[1..].to_owned());
        names.push(keyword[..keyword.len() - 1].to_owned());
        for i in 0..keyword.len() {
            let mut bytes = keyword.as_bytes().to_vec();
            bytes[i] ^= 1;
            names.push(String::from_utf8(bytes).expect("ASCII stays UTF-8"));
        }
    }
    for name in names.iter().filter(|name| !name.is_empty()) {
        let expected = TokenKind::keyword(name).unwrap_or(TokenKind::Identifier);
        for after in [b'\0', b' ', b'e', b'(', 0xFF] {
            let mut first_eight = [after; 8];
            let shown = name.len().min(8);
            first_eight[..shown].copy_from_slice(&name.as_bytes()[..shown]);
            let kind = TokenKind::of_name(u64::from_le_bytes(first_eight), name.len());
            assert_eq!(kind, expected, "{name:?} then {after:#04x}");
        }
    }
}

#[test]
fn is_reserved_holds_for_keywords_only() {
    assert!(TokenKind::is_reserved("while"));
    let names = ["", "x", "continue", "type", "export", "const", "typeof", "While"];
    for name in names {
        assert!(!TokenKind::is_reserved(name), "{name:?}");
    }
}

#[test]
fn is_identifier_shaped_holds_for_ascii_names_keywords_included() {
    for text in ["x", "_", "_private", "snake_case2", "camelCase", "end", "continue"] {
        assert!(TokenKind::is_identifier_shaped(text), "{text:?}");
    }
    for text in ["", "1st", "a-b", "has space", "a.b", "a\\nb", "\u{dc}nicode", "x\u{e9}"] {
        assert!(!TokenKind::is_identifier_shaped(text), "{text:?}");
    }
}
