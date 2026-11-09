use luck_token::{CompactString, LuaVersion, Span, Token, TokenKind};

// `lex` returns every token in one buffer, so a size regression costs
// memory on every file. `CompactString` (24) plus tag padding puts
// `TokenKind` at 32, and `Span` (8) brings `Token` to 40.
#[test]
#[cfg(target_pointer_width = "64")]
fn token_size_is_pinned() {
    assert_eq!(size_of::<TokenKind>(), 32);
    assert_eq!(size_of::<Token>(), 40);
    assert_eq!(size_of::<Option<Token>>(), 40);
}

#[test]
fn compact_string_keeps_short_identifiers_inline() {
    // The README promises that short strings skip the heap.
    assert!(!CompactString::from("short_ident").is_heap_allocated());
    assert!(CompactString::from("x".repeat(64)).is_heap_allocated());
}

#[test]
fn display_shows_identifier_and_number_text_but_not_string_text() {
    assert_eq!(TokenKind::Identifier(CompactString::from("foo")).to_string(), "identifier `foo`");
    assert_eq!(TokenKind::Number(CompactString::from("42")).to_string(), "number `42`");
    // The string payload is deliberately not echoed.
    assert_eq!(TokenKind::StringLiteral(CompactString::from("secret")).to_string(), "string");
    assert_eq!(
        TokenKind::InterpolatedStringMiddle(CompactString::from("secret")).to_string(),
        "interpolated string middle"
    );
}

#[test]
fn display_quotes_symbols_and_keywords() {
    assert_eq!(TokenKind::And.to_string(), "`and`");
    assert_eq!(TokenKind::DotDotDot.to_string(), "`...`");
    assert_eq!(TokenKind::LeftBrace.to_string(), "`{`");
    assert_eq!(TokenKind::RightBrace.to_string(), "`}`");
    assert_eq!(TokenKind::Arrow.to_string(), "`->`");
    assert_eq!(TokenKind::Eof.to_string(), "end of file");
}

#[test]
fn static_text_is_none_only_for_text_carrying_kinds_and_eof() {
    assert_eq!(TokenKind::Global.static_text(), Some("global"));
    assert_eq!(TokenKind::DoubleColon.static_text(), Some("::"));
    assert_eq!(TokenKind::Eof.static_text(), None);
    assert_eq!(TokenKind::Identifier(CompactString::from("end")).static_text(), None);
    assert_eq!(TokenKind::InterpolatedStringBegin(CompactString::from("x")).static_text(), None);
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
        for kind in &base_keywords {
            let text = kind.static_text().unwrap_or_default();
            assert_eq!(TokenKind::keyword(text, version).as_ref(), Some(kind));
        }
        let goto = version.has_goto().then_some(TokenKind::Goto);
        assert_eq!(TokenKind::keyword("goto", version), goto, "{version:?}");
        let global = version.has_global().then_some(TokenKind::Global);
        assert_eq!(TokenKind::keyword("global", version), global, "{version:?}");
        for name in ["", "x", "End", "functions", "continue", "type"] {
            assert_eq!(TokenKind::keyword(name, version), None, "{name:?}");
        }
    }
}

#[test]
fn is_reserved_adds_luau_continue_to_each_version_keywords() {
    for version in LuaVersion::ALL {
        assert!(TokenKind::is_reserved("while", version), "{version:?}");
        assert_eq!(TokenKind::is_reserved("goto", version), version.has_goto(), "{version:?}");
        assert_eq!(TokenKind::is_reserved("global", version), version.has_global(), "{version:?}");
        assert_eq!(
            TokenKind::is_reserved("continue", version),
            version.has_continue(),
            "{version:?}"
        );
        for name in ["", "x", "type", "export", "While"] {
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
fn identifier_reads_only_identifier_names() {
    let span = Span::new(0, 3);
    let name = Token::new(TokenKind::Identifier(CompactString::from("foo")), span);
    assert_eq!(name.identifier(), Some("foo"));
    let string = Token::new(TokenKind::StringLiteral(CompactString::from("\"foo\"")), span);
    assert_eq!(string.identifier(), None);
    assert_eq!(Token::new(TokenKind::Nil, span).identifier(), None);
}

#[test]
fn bug_global_is_a_name_on_default_lua55_builds() {
    assert_eq!(
        TokenKind::keyword("global", LuaVersion::Lua55),
        None,
        "stock Lua 5.5 defines LUA_COMPAT_GLOBAL, so `global` is a regular name and `local global = 1` compiles"
    );
}
