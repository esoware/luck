use luck_lexer::lex;
use luck_token::{LuaVersion, Span, TokenKind};

use crate::common::{first_kind, first_kind_in, kinds, kinds_in, lex_lua51};

#[test]
fn lua51_keywords_lex_as_keyword_tokens() {
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
    for source in ["foo", "_bar", "_123", "abc_def"] {
        assert_eq!(first_kind(source), TokenKind::Identifier(source.into()));
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
        // '#' at the very start of a chunk is a first-line comment (like
        // luaL_loadfilex); lex it after a newline to get the length op.
        ("\n#", TokenKind::Hash),
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
    ];
    for (source, expected) in cases {
        assert_eq!(first_kind(source), expected, "failed for: {source:?}");
    }
}

#[test]
fn whitespace_yields_no_tokens() {
    assert_eq!(
        kinds("  x  +  y  "),
        vec![TokenKind::Identifier("x".into()), TokenKind::Plus, TokenKind::Identifier("y".into()),]
    );
}

#[test]
fn token_stream_ends_with_eof() {
    let result = lex_lua51("x");
    assert_eq!(result.tokens.last().expect("tokens should not be empty").kind, TokenKind::Eof);
}

#[test]
fn empty_source_lexes_to_only_eof() {
    let result = lex_lua51("");
    assert_eq!(result.tokens.len(), 1);
    assert_eq!(result.tokens[0].kind, TokenKind::Eof);
}

#[test]
fn token_spans_cover_their_source_bytes() {
    let result = lex_lua51("local x = 42");
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
            TokenKind::Identifier("x".into()),
            TokenKind::Equal,
            TokenKind::Number("1".into()),
            TokenKind::Plus,
            TokenKind::Number("2".into()),
        ]
    );
}

#[test]
fn call_with_string_argument_lexes_token_by_token() {
    assert_eq!(
        kinds("print(\"hello\")"),
        vec![
            TokenKind::Identifier("print".into()),
            TokenKind::LeftParen,
            TokenKind::StringLiteral("\"hello\"".into()),
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
            TokenKind::Number("1".into()),
            TokenKind::Comma,
            TokenKind::Number("2".into()),
            TokenKind::Comma,
            TokenKind::Number("3".into()),
            TokenKind::RightBrace,
        ]
    );
}

#[test]
fn field_and_method_access_lex_token_by_token() {
    assert_eq!(
        kinds("a.b:c()"),
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Dot,
            TokenKind::Identifier("b".into()),
            TokenKind::Colon,
            TokenKind::Identifier("c".into()),
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
fn bom_is_skipped() {
    let result = lex("\u{FEFF}local x = 1", LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert_eq!(result.tokens[0].kind, TokenKind::Local);
}

#[test]
fn non_ascii_unexpected_char_is_one_error() {
    let result = lex("local caf\u{e9} = 1", LuaVersion::Lua54);
    assert_eq!(result.errors.len(), 1, "errors: {:?}", result.errors);
    assert!(result.errors[0].message.contains('\u{e9}'));
}

#[test]
fn goto_is_a_keyword_only_in_lua52_plus() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            first_kind_in("goto", version),
            TokenKind::Goto,
            "goto should be keyword in {version:?}"
        );
    }
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        assert_eq!(
            first_kind_in("goto", version),
            TokenKind::Identifier("goto".into()),
            "goto should be identifier in {version:?}"
        );
    }
}

#[test]
fn global_is_a_name_in_every_version() {
    for version in LuaVersion::ALL {
        assert_eq!(
            first_kind_in("global", version),
            TokenKind::Identifier("global".into()),
            "global should be identifier in {version:?}"
        );
    }
}

#[test]
fn lua52_goto_and_label_lex_token_by_token() {
    assert_eq!(
        kinds_in("goto done :: done ::", LuaVersion::Lua52),
        vec![
            TokenKind::Goto,
            TokenKind::Identifier("done".into()),
            TokenKind::DoubleColon,
            TokenKind::Identifier("done".into()),
            TokenKind::DoubleColon,
        ]
    );
}

#[test]
fn lua55_global_stmt_lexes_token_by_token() {
    assert_eq!(
        kinds_in("global x = 5", LuaVersion::Lua55),
        vec![
            TokenKind::Identifier("global".into()),
            TokenKind::Identifier("x".into()),
            TokenKind::Equal,
            TokenKind::Number("5".into()),
        ]
    );
}

#[test]
fn floor_div_lexes_only_in_lua53_plus_and_luau() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        assert_eq!(
            first_kind_in("//", version),
            TokenKind::FloorDiv,
            "// should be FloorDiv in {version:?}"
        );
    }
    for version in [LuaVersion::Lua51, LuaVersion::Lua52] {
        assert_eq!(
            kinds_in("//", version),
            vec![TokenKind::Slash, TokenKind::Slash],
            "// should be two slashes in {version:?}"
        );
    }
}

#[test]
fn floor_div_not_confused_with_comment() {
    assert_eq!(kinds_in("-- comment\n//", LuaVersion::Lua53), vec![TokenKind::FloorDiv]);
}

#[test]
fn lua52_double_slash_lexes_as_two_slashes() {
    assert_eq!(
        kinds_in("a // b", LuaVersion::Lua52),
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Slash,
            TokenKind::Slash,
            TokenKind::Identifier("b".into()),
        ]
    );
}

#[test]
fn standalone_tilde_lexes_only_in_lua53_plus() {
    // Lua 5.3+ supports bitwise NOT; Luau doesn't.
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let result = lex("~", version);
        assert!(result.errors.is_empty(), "~ should work in {version:?}");
        assert_eq!(result.tokens[0].kind, TokenKind::Tilde);
    }
    for version in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Luau] {
        let result = lex("~", version);
        assert!(!result.errors.is_empty(), "standalone ~ should error in {version:?}");
    }
}

#[test]
fn tilde_equal_lexes_in_every_version() {
    for version in LuaVersion::ALL {
        let result = lex("~=", version);
        assert!(result.errors.is_empty(), "~= failed in {version:?}");
        assert_eq!(result.tokens[0].kind, TokenKind::TildeEqual);
    }
}

#[test]
fn ampersand_and_pipe_lex_only_in_lua53_plus_and_luau() {
    // Luau uses `&` and `|` for intersection and union types.
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        assert_eq!(
            first_kind_in("&", version),
            TokenKind::Ampersand,
            "& should work in {version:?}"
        );
        assert_eq!(first_kind_in("|", version), TokenKind::Pipe, "| should work in {version:?}");
    }
    for version in [LuaVersion::Lua51, LuaVersion::Lua52] {
        assert!(!lex("&", version).errors.is_empty(), "& should error in {version:?}");
        assert!(!lex("|", version).errors.is_empty(), "| should error in {version:?}");
    }
}

#[test]
fn shift_operators_lex_only_in_lua53_plus() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(first_kind_in("<<", version), TokenKind::ShiftLeft, "<< in {version:?}");
        assert_eq!(first_kind_in(">>", version), TokenKind::ShiftRight, ">> in {version:?}");
    }
    // Without bitwise ops, `<<` and `>>` are two separate tokens.
    assert_eq!(kinds_in("<<", LuaVersion::Lua51), vec![TokenKind::Less, TokenKind::Less]);
    assert_eq!(kinds_in(">>", LuaVersion::Lua51), vec![TokenKind::Greater, TokenKind::Greater]);
}

#[test]
fn lua53_bitwise_expr_lexes_token_by_token() {
    assert_eq!(
        kinds_in("a & b | c", LuaVersion::Lua53),
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Ampersand,
            TokenKind::Identifier("b".into()),
            TokenKind::Pipe,
            TokenKind::Identifier("c".into()),
        ]
    );
}

#[test]
fn lua53_shift_expr_lexes_token_by_token() {
    assert_eq!(
        kinds_in("x << 2 >> 1", LuaVersion::Lua53),
        vec![
            TokenKind::Identifier("x".into()),
            TokenKind::ShiftLeft,
            TokenKind::Number("2".into()),
            TokenKind::ShiftRight,
            TokenKind::Number("1".into()),
        ]
    );
}

#[test]
fn lua53_mixed_bitwise_expr_lexes_token_by_token() {
    assert_eq!(
        kinds_in("(a & b) | (~c >> 2)", LuaVersion::Lua53),
        vec![
            TokenKind::LeftParen,
            TokenKind::Identifier("a".into()),
            TokenKind::Ampersand,
            TokenKind::Identifier("b".into()),
            TokenKind::RightParen,
            TokenKind::Pipe,
            TokenKind::LeftParen,
            TokenKind::Tilde,
            TokenKind::Identifier("c".into()),
            TokenKind::ShiftRight,
            TokenKind::Number("2".into()),
            TokenKind::RightParen,
        ]
    );
}

#[test]
fn lua54_const_attribute_lexes_as_separate_tokens() {
    // The parser handles `<const>`; the lexer sees plain angle brackets.
    assert_eq!(
        kinds_in("local x <const> = 5", LuaVersion::Lua54),
        vec![
            TokenKind::Local,
            TokenKind::Identifier("x".into()),
            TokenKind::Less,
            TokenKind::Identifier("const".into()),
            TokenKind::Greater,
            TokenKind::Equal,
            TokenKind::Number("5".into()),
        ]
    );
}

#[test]
fn lua54_close_attribute_lexes_as_separate_tokens() {
    let token_kinds = kinds_in("local f <close> = io.open('x')", LuaVersion::Lua54);
    assert!(token_kinds.contains(&TokenKind::Less));
    assert!(token_kinds.contains(&TokenKind::Identifier("close".into())));
    assert!(token_kinds.contains(&TokenKind::Greater));
}

#[test]
fn compound_assignment_operators_lex_only_in_luau() {
    let operators = [
        ("+=", TokenKind::PlusEqual),
        ("-=", TokenKind::MinusEqual),
        ("*=", TokenKind::StarEqual),
        ("/=", TokenKind::SlashEqual),
        ("//=", TokenKind::FloorDivEqual),
        ("%=", TokenKind::PercentEqual),
        ("^=", TokenKind::CaretEqual),
        ("..=", TokenKind::DotDotEqual),
    ];
    for (source, expected) in &operators {
        assert_eq!(
            first_kind_in(source, LuaVersion::Luau),
            *expected,
            "{source} should be compound op in Luau"
        );
    }

    // Outside Luau, these split into two tokens.
    let split_cases: &[(&str, LuaVersion, &[TokenKind])] = &[
        ("+=", LuaVersion::Lua54, &[TokenKind::Plus, TokenKind::Equal]),
        ("-=", LuaVersion::Lua54, &[TokenKind::Minus, TokenKind::Equal]),
        ("*=", LuaVersion::Lua53, &[TokenKind::Star, TokenKind::Equal]),
        ("/=", LuaVersion::Lua51, &[TokenKind::Slash, TokenKind::Equal]),
        ("%=", LuaVersion::Lua52, &[TokenKind::Percent, TokenKind::Equal]),
        ("^=", LuaVersion::Lua51, &[TokenKind::Caret, TokenKind::Equal]),
        ("..=", LuaVersion::Lua54, &[TokenKind::DotDot, TokenKind::Equal]),
        ("//=", LuaVersion::Lua53, &[TokenKind::FloorDiv, TokenKind::Equal]),
    ];
    for (source, version, expected) in split_cases {
        assert_eq!(kinds_in(source, *version), *expected, "{source} should split in {version:?}");
    }
}

#[test]
fn at_lexes_only_in_luau() {
    assert_eq!(first_kind_in("@", LuaVersion::Luau), TokenKind::At);
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert!(!lex("@", version).errors.is_empty(), "@ should error in {version:?}");
    }
}

#[test]
fn luau_attribute_lexes_as_at_and_a_name() {
    assert_eq!(
        kinds_in("@native", LuaVersion::Luau),
        vec![TokenKind::At, TokenKind::Identifier("native".into())]
    );
}

#[test]
fn luau_nul_byte_ends_the_source() {
    let result = lex("return 1\0$", LuaVersion::Luau);
    assert!(
        result.errors.is_empty(),
        "Luau reads a raw NUL as end of input, so the bytes after it should not be lexed: {:?}",
        result.errors
    );
    assert_eq!(result.tokens.len(), 3, "{:?}", result.tokens);
}

#[test]
fn bom_is_an_error_on_lua51_and_luau() {
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        let result = lex("\u{FEFF}return 1", version);
        assert!(
            !result.errors.is_empty(),
            "a leading BOM fails to compile under {version:?}, so it should be reported: {:?}",
            result.tokens
        );
    }
}
