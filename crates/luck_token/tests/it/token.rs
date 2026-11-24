use luck_token::{LuaVersion, Span, Token, TokenKind};

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
    assert_eq!(TokenKind::Goto.static_text(), Some("goto"));
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

#[test]
fn keyword_reads_back_each_keyword_spelling_under_its_versions() {
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
    for version in LuaVersion::ALL {
        for kind in base_keywords {
            let text = kind.static_text().unwrap_or_default();
            assert_eq!(TokenKind::keyword(text, version), Some(kind));
        }
        let goto = version.has_goto().then_some(TokenKind::Goto);
        assert_eq!(TokenKind::keyword("goto", version), goto, "{version:?}");
        for name in ["", "x", "End", "functions", "types", "Global"] {
            assert_eq!(TokenKind::keyword(name, version), None, "{name:?}");
        }
    }
}

#[test]
fn keyword_reads_each_contextual_keyword_only_where_its_version_has_it() {
    for version in LuaVersion::ALL {
        for (text, kind, has_it) in [
            ("continue", TokenKind::Continue, version.has_continue()),
            ("export", TokenKind::Export, version.has_type_syntax()),
            ("const", TokenKind::Const, version.has_const_stmt()),
            ("type", TokenKind::Type, version.has_type_syntax()),
            ("typeof", TokenKind::Typeof, version.has_type_syntax()),
            ("read", TokenKind::Read, version.has_type_syntax()),
            ("write", TokenKind::Write, version.has_type_syntax()),
            ("global", TokenKind::Global, version.has_global()),
        ] {
            assert_eq!(kind.static_text(), Some(text));
            assert!(kind.is_name(), "{kind:?}");
            assert_eq!(TokenKind::keyword(text, version), has_it.then_some(kind), "{version:?}");
        }
    }
}

/// A name that hashes to a keyword's slot is still a name, so a lookup has to
/// compare the whole spelling.
#[test]
fn keyword_rejects_every_near_spelling_of_a_keyword() {
    let keywords = [
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
        "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
        "continue", "export", "const", "type", "typeof", "read", "write", "global",
    ];
    for keyword in keywords {
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
        for name in near.iter().filter(|name| !keywords.contains(&name.as_str())) {
            for version in LuaVersion::ALL {
                assert_eq!(TokenKind::keyword(name, version), None, "{name:?}");
            }
        }
    }
}

#[test]
fn is_reserved_holds_for_keywords_and_lua55_global_only() {
    for version in LuaVersion::ALL {
        assert!(TokenKind::is_reserved("while", version), "{version:?}");
        assert_eq!(TokenKind::is_reserved("goto", version), version.has_goto(), "{version:?}");
        assert_eq!(TokenKind::is_reserved("global", version), version.has_global(), "{version:?}");
        for name in ["", "x", "continue", "type", "export", "const", "typeof", "While"] {
            assert!(!TokenKind::is_reserved(name, version), "{name:?} {version:?}");
        }
    }
}

#[test]
fn is_identifier_shaped_holds_for_ascii_names_keywords_included() {
    for text in ["x", "_", "_private", "snake_case2", "camelCase", "end", "goto"] {
        assert!(TokenKind::is_identifier_shaped(text), "{text:?}");
    }
    for text in ["", "1st", "a-b", "has space", "a.b", "a\\nb", "\u{dc}nicode", "x\u{e9}"] {
        assert!(!TokenKind::is_identifier_shaped(text), "{text:?}");
    }
}

#[test]
fn global_is_a_name_on_default_lua55_builds() {
    assert!(
        TokenKind::keyword("global", LuaVersion::Lua55).is_some_and(TokenKind::is_name),
        "stock Lua 5.5 defines LUA_COMPAT_GLOBAL, so `global` is a regular name and `local global = 1` compiles"
    );
}
