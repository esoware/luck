use luck_lexer::lex;
use luck_token::*;

use crate::common::{first_kind, first_kind_v, kinds_v};

#[test]
fn tilde_standalone_version_gating() {
    // Tilde is bitwise NOT in 5.3-5.5
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let result = lex("~", version);
        assert!(result.errors.is_empty(), "~ should work in {:?}", version);
        assert_eq!(result.tokens[0].kind, TokenKind::Tilde);
    }
    // Error in all other versions
    for version in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Luau] {
        let result = lex("~", version);
        assert!(
            !result.errors.is_empty(),
            "standalone ~ should error in {:?}",
            version
        );
    }
}

#[test]
fn goto_version_gating() {
    for version in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert_eq!(
            first_kind_v("goto", version),
            TokenKind::Goto,
            "goto should be keyword in {:?}",
            version
        );
    }
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        assert_eq!(
            first_kind_v("goto", version),
            TokenKind::Identifier("goto".into()),
            "goto should be identifier in {:?}",
            version
        );
    }
}

#[test]
fn floor_div_version_gating() {
    for version in [
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        assert_eq!(
            first_kind_v("//", version),
            TokenKind::FloorDiv,
            "// should be FloorDiv in {:?}",
            version
        );
    }
    for version in [LuaVersion::Lua51, LuaVersion::Lua52] {
        let ks = kinds_v("//", version);
        assert_eq!(
            ks,
            vec![TokenKind::Slash, TokenKind::Slash],
            "// should be two slashes in {:?}",
            version
        );
    }
}

#[test]
fn floor_div_not_confused_with_comment() {
    // -- is a comment, // is floor div; they should not interact
    let result = lex("-- comment\n//", LuaVersion::Lua53);
    assert!(result.errors.is_empty());
    let non_eof: Vec<_> = result
        .tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .map(|t| &t.kind)
        .collect();
    assert_eq!(non_eof, vec![&TokenKind::FloorDiv]);
}

#[test]
fn hex_escape_incomplete() {
    let result = lex("\"\\xG1\"", LuaVersion::Lua52);
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("hex digit"));
}

#[test]
fn unicode_escape_empty_braces() {
    let result = lex("\"\\u{}\"", LuaVersion::Lua53);
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("at least one hex digit"));
}

#[test]
fn unicode_escape_missing_brace() {
    let result = lex("\"\\u41}\"", LuaVersion::Lua53);
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("'{'"));
}

#[test]
fn hex_float_formats() {
    let cases = [
        ("0x1.Fp10", LuaVersion::Lua52, "0x1.Fp10"),
        ("0xAp4", LuaVersion::Lua53, "0xAp4"),
        ("0xA23p-4", LuaVersion::Lua52, "0xA23p-4"),
        ("0x1.0p+3", LuaVersion::Lua54, "0x1.0p+3"),
    ];
    for (source, version, expected) in cases {
        assert_eq!(
            first_kind_v(source, version),
            TokenKind::Number(expected.into()),
            "failed for: {} in {:?}",
            source,
            version
        );
    }
}

#[test]
fn hex_float_rejected_in_unsupported_versions() {
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        let result = lex("0x1.Fp10", version);
        assert!(
            !result.errors.is_empty(),
            "hex float should error in {:?}",
            version
        );
    }
}

#[test]
fn hex_float_requires_exponent() {
    let result = lex("0x1.F", LuaVersion::Lua52);
    assert!(!result.errors.is_empty());
}

#[test]
fn ampersand_and_pipe_version_gating() {
    // Luau uses & and | for intersection/union types
    for version in [
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        assert_eq!(
            first_kind_v("&", version),
            TokenKind::Ampersand,
            "& should work in {:?}",
            version
        );
    }
    for version in [
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        assert_eq!(
            first_kind_v("|", version),
            TokenKind::Pipe,
            "| should work in {:?}",
            version
        );
    }
    for version in [LuaVersion::Lua51, LuaVersion::Lua52] {
        assert!(
            !lex("&", version).errors.is_empty(),
            "& should error in {:?}",
            version
        );
        assert!(
            !lex("|", version).errors.is_empty(),
            "| should error in {:?}",
            version
        );
    }
}

#[test]
fn shift_operators_version_gating() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            first_kind_v("<<", version),
            TokenKind::ShiftLeft,
            "<< in {:?}",
            version
        );
        assert_eq!(
            first_kind_v(">>", version),
            TokenKind::ShiftRight,
            ">> in {:?}",
            version
        );
    }
    // In versions without bitwise ops, << and >> are two separate tokens
    let ks = kinds_v("<<", LuaVersion::Lua51);
    assert_eq!(ks, vec![TokenKind::Less, TokenKind::Less]);
    let ks = kinds_v(">>", LuaVersion::Lua51);
    assert_eq!(ks, vec![TokenKind::Greater, TokenKind::Greater]);
}

#[test]
fn tilde_equal_works_in_all_versions() {
    // ~= should always work, even in Lua 5.1
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        let result = lex("~=", version);
        assert!(result.errors.is_empty(), "~= failed in {:?}", version);
        assert_eq!(result.tokens[0].kind, TokenKind::TildeEqual);
    }
}

#[test]
fn lua53_bitwise_expression() {
    let ks = kinds_v("a & b | c", LuaVersion::Lua53);
    assert_eq!(
        ks,
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
fn lua53_shift_expression() {
    let ks = kinds_v("x << 2 >> 1", LuaVersion::Lua53);
    assert_eq!(
        ks,
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
fn lua54_attributes_lex_as_separate_tokens() {
    // <const> is parsed by the parser, not the lexer
    let ks = kinds_v("local x <const> = 5", LuaVersion::Lua54);
    assert_eq!(
        ks,
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
fn lua54_close_attribute() {
    let ks = kinds_v("local f <close> = io.open('x')", LuaVersion::Lua54);
    assert!(ks.contains(&TokenKind::Less));
    assert!(ks.contains(&TokenKind::Identifier("close".into())));
    assert!(ks.contains(&TokenKind::Greater));
}

#[test]
fn global_version_gating() {
    assert_eq!(first_kind_v("global", LuaVersion::Lua55), TokenKind::Global);
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Luau,
    ] {
        assert_eq!(
            first_kind_v("global", version),
            TokenKind::Identifier("global".into()),
            "global should be identifier in {:?}",
            version
        );
    }
}

#[test]
fn compound_assignment_operators_luau_only() {
    let ops = [
        ("+=", TokenKind::PlusEqual),
        ("-=", TokenKind::MinusEqual),
        ("*=", TokenKind::StarEqual),
        ("/=", TokenKind::SlashEqual),
        ("//=", TokenKind::FloorDivEqual),
        ("%=", TokenKind::PercentEqual),
        ("^=", TokenKind::CaretEqual),
        ("..=", TokenKind::DotDotEqual),
    ];
    for (src, expected) in &ops {
        assert_eq!(
            first_kind_v(src, LuaVersion::Luau),
            *expected,
            "{} should be compound op in Luau",
            src
        );
    }

    // In non-Luau versions, these split into two tokens
    let split_cases: &[(&str, LuaVersion, &[TokenKind])] = &[
        (
            "+=",
            LuaVersion::Lua54,
            &[TokenKind::Plus, TokenKind::Equal],
        ),
        (
            "-=",
            LuaVersion::Lua54,
            &[TokenKind::Minus, TokenKind::Equal],
        ),
        (
            "*=",
            LuaVersion::Lua53,
            &[TokenKind::Star, TokenKind::Equal],
        ),
        (
            "/=",
            LuaVersion::Lua51,
            &[TokenKind::Slash, TokenKind::Equal],
        ),
        (
            "%=",
            LuaVersion::Lua52,
            &[TokenKind::Percent, TokenKind::Equal],
        ),
    ];
    for (src, version, expected) in split_cases {
        let ks = kinds_v(src, *version);
        assert_eq!(ks, *expected, "{} should split in {:?}", src, version);
    }
}

#[test]
fn at_token_luau_only() {
    assert_eq!(first_kind_v("@", LuaVersion::Luau), TokenKind::At);
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert!(
            !lex("@", version).errors.is_empty(),
            "@ should error in {:?}",
            version
        );
    }
}

#[test]
fn binary_literal_luau_only() {
    assert_eq!(
        first_kind_v("0b1010", LuaVersion::Luau),
        TokenKind::Number("0b1010".into())
    );
    assert_eq!(
        first_kind_v("0B11", LuaVersion::Luau),
        TokenKind::Number("0B11".into())
    );
    // In non-Luau, "0b1010" splits into number "0" + identifier "b1010"
    let ks = kinds_v("0b1010", LuaVersion::Lua54);
    assert_eq!(ks[0], TokenKind::Number("0".into()));
    assert_eq!(ks[1], TokenKind::Identifier("b1010".into()));
}

#[test]
fn underscore_separator_luau() {
    let cases = [
        ("1_000_000", "1_000_000"),
        ("0xFF_FF", "0xFF_FF"),
        ("0b1010_0101", "0b1010_0101"),
    ];
    for (source, expected) in cases {
        assert_eq!(
            first_kind_v(source, LuaVersion::Luau),
            TokenKind::Number(expected.into()),
            "failed for: {}",
            source
        );
    }

    assert!(
        !lex("0x_FF", LuaVersion::Luau).errors.is_empty(),
        "underscore at start"
    );
    assert!(
        !lex("100_", LuaVersion::Luau).errors.is_empty(),
        "underscore at end"
    );

    // In non-Luau, splits into number + identifier
    let result = lex("1_000", LuaVersion::Lua54);
    let non_eof: Vec<_> = result
        .tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .collect();
    assert_eq!(non_eof[0].kind, TokenKind::Number("1".into()));
    assert_eq!(non_eof[1].kind, TokenKind::Identifier("_000".into()));
}

#[test]
fn interp_string_no_expressions_luau() {
    let result = lex("`hello`", LuaVersion::Luau);
    assert!(result.errors.is_empty());
    let non_eof: Vec<_> = result
        .tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .map(|t| &t.kind)
        .collect();
    assert_eq!(
        non_eof,
        vec![
            &TokenKind::InterpBegin(CompactString::default()),
            &TokenKind::InterpEnd("hello".into()),
        ]
    );
}

#[test]
fn interp_string_with_expression_luau() {
    // `hello {` - the lexer stops at { and emits InterpBegin
    let result = lex("`hello {", LuaVersion::Luau);
    assert!(result.errors.is_empty());
    assert_eq!(
        result.tokens[0].kind,
        TokenKind::InterpBegin("hello ".into())
    );
}

#[test]
fn interp_string_empty_luau() {
    let result = lex("``", LuaVersion::Luau);
    assert!(result.errors.is_empty());
    let non_eof: Vec<_> = result
        .tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .map(|t| &t.kind)
        .collect();
    assert_eq!(
        non_eof,
        vec![
            &TokenKind::InterpBegin(CompactString::default()),
            &TokenKind::InterpEnd(CompactString::default()),
        ]
    );
}

#[test]
fn interp_string_unterminated_luau() {
    let result = lex("`hello", LuaVersion::Luau);
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("unterminated"));
}

#[test]
fn backtick_error_in_lua54() {
    let result = lex("`hello`", LuaVersion::Lua54);
    assert!(!result.errors.is_empty());
}

#[test]
fn hex_escape_all_supported_versions() {
    for version in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        let result = lex("\"\\x41\"", version);
        assert!(
            result.errors.is_empty(),
            "\\x41 should work in {:?}",
            version
        );
    }
}

#[test]
fn whitespace_escape_all_supported_versions() {
    for version in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        let result = lex("\"\\z \"", version);
        assert!(result.errors.is_empty(), "\\z should work in {:?}", version);
    }
}

#[test]
fn unicode_escape_only_53_plus_and_luau() {
    for version in [
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        let result = lex("\"\\u{41}\"", version);
        assert!(
            result.errors.is_empty(),
            "\\u{{41}} should work in {:?}",
            version
        );
    }
    for version in [LuaVersion::Lua51, LuaVersion::Lua52] {
        let result = lex("\"\\u{41}\"", version);
        assert!(
            !result.errors.is_empty(),
            "\\u{{41}} should fail in {:?}",
            version
        );
    }
}

#[test]
fn hex_float_all_supported_versions() {
    for version in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let result = lex("0x1.Fp10", version);
        assert!(
            result.errors.is_empty(),
            "hex float should work in {:?}",
            version
        );
    }
}

#[test]
fn lua52_floor_div_in_expression() {
    let ks = kinds_v("a // b", LuaVersion::Lua52);
    assert_eq!(
        ks,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Slash,
            TokenKind::Slash,
            TokenKind::Identifier("b".into()),
        ]
    );
}

#[test]
fn lua53_full_expression_with_bitwise() {
    let ks = kinds_v("(a & b) | (~c >> 2)", LuaVersion::Lua53);
    assert_eq!(
        ks,
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
fn lua55_global_statement() {
    let ks = kinds_v("global x = 5", LuaVersion::Lua55);
    assert_eq!(
        ks,
        vec![
            TokenKind::Global,
            TokenKind::Identifier("x".into()),
            TokenKind::Equal,
            TokenKind::Number("5".into()),
        ]
    );
}

#[test]
fn lua52_goto_label() {
    let ks = kinds_v("goto done :: done ::", LuaVersion::Lua52);
    assert_eq!(
        ks,
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
fn hex_integer_still_works_in_all_versions() {
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        let result = lex("0xFF", version);
        assert!(
            result.errors.is_empty(),
            "0xFF should work in {:?}",
            version
        );
        assert_eq!(result.tokens[0].kind, TokenKind::Number("0xFF".into()));
    }
}

#[test]
fn decimal_numbers_work_in_all_versions() {
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
        LuaVersion::Luau,
    ] {
        let result = lex("3.14e2", version);
        assert!(
            result.errors.is_empty(),
            "3.14e2 should work in {:?}",
            version
        );
    }
}

#[test]
fn luau_at_in_context() {
    let ks = kinds_v("@native", LuaVersion::Luau);
    assert_eq!(
        ks,
        vec![TokenKind::At, TokenKind::Identifier("native".into()),]
    );
}

#[test]
fn interp_string_with_escape_luau() {
    let result = lex("`hello\\nworld`", LuaVersion::Luau);
    assert!(result.errors.is_empty());
    let non_eof: Vec<_> = result
        .tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .map(|t| &t.kind)
        .collect();
    assert_eq!(non_eof[1], &TokenKind::InterpEnd("hello\\nworld".into()));
}

#[test]
fn compound_ops_split_in_non_luau() {
    let cases: &[(&str, LuaVersion, &[TokenKind])] = &[
        (
            "^=",
            LuaVersion::Lua51,
            &[TokenKind::Caret, TokenKind::Equal],
        ),
        (
            "..=",
            LuaVersion::Lua54,
            &[TokenKind::DotDot, TokenKind::Equal],
        ),
        (
            "//=",
            LuaVersion::Lua53,
            &[TokenKind::FloorDiv, TokenKind::Equal],
        ),
    ];
    for (src, version, expected) in cases {
        let ks = kinds_v(src, *version);
        assert_eq!(ks, *expected, "{} should split in {:?}", src, version);
    }
}

#[test]
fn luau_underscores_in_various_number_bases() {
    let cases = [
        ("0b1111_0000", "0b1111_0000"),
        ("1_0e1_0", "1_0e1_0"),
        ("0xDEAD_BEEF", "0xDEAD_BEEF"),
    ];
    for (source, expected) in cases {
        assert_eq!(
            first_kind_v(source, LuaVersion::Luau),
            TokenKind::Number(expected.into()),
            "failed for: {}",
            source
        );
    }
}

#[test]
fn hex_no_digits_is_error() {
    let result = lex("0x", LuaVersion::Lua54);
    assert!(!result.errors.is_empty());
    assert!(
        result.errors[0]
            .message
            .contains("hex literal requires at least one digit")
    );
}

#[test]
fn trailing_dot_floats() {
    let cases = [("1.", "1."), ("1.e5", "1.e5"), ("0.", "0.")];
    for (source, expected) in cases {
        assert_eq!(
            first_kind(source),
            TokenKind::Number(expected.into()),
            "failed for: {}",
            source
        );
    }
}

#[test]
fn unicode_escape_codepoint_bounds() {
    let result = lex(r#""\u{7FFFFFFF}""#, LuaVersion::Lua54);
    assert!(result.errors.is_empty());
    assert_eq!(
        result.tokens[0].kind,
        TokenKind::StringLiteral(r#""\u{7FFFFFFF}""#.into())
    );

    for overflow in [r#""\u{80000000}""#, r#""\u{FFFFFFFF}""#] {
        let result = lex(overflow, LuaVersion::Lua54);
        assert!(!result.errors.is_empty(), "{} should overflow", overflow);
        assert!(result.errors[0].message.contains("codepoint too large"));
    }
}

#[test]
fn binary_no_digits_is_error() {
    let result = lex("0b", LuaVersion::Luau);
    assert!(!result.errors.is_empty());
    assert!(
        result.errors[0]
            .message
            .contains("binary literal requires at least one digit")
    );
}

#[test]
fn interp_string_nested_index_expression() {
    let result = lex("`{t[1]}`", LuaVersion::Luau);
    assert!(result.errors.is_empty());
    let non_eof: Vec<_> = result
        .tokens
        .iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .map(|t| &t.kind)
        .collect();
    assert_eq!(
        non_eof[0],
        &TokenKind::InterpBegin(CompactString::default())
    );
    assert_eq!(non_eof[1], &TokenKind::Identifier("t".into()));
    assert_eq!(non_eof[2], &TokenKind::LeftBracket);
    assert_eq!(non_eof[3], &TokenKind::Number("1".into()));
    assert_eq!(non_eof[4], &TokenKind::RightBracket);
    assert_eq!(non_eof[5], &TokenKind::InterpEnd(CompactString::default()));
}

#[test]
fn luau_double_brace_in_interp_string() {
    let result = lex("`{{x}}`", LuaVersion::Luau);
    assert!(!result.errors.is_empty());
    assert!(result.errors[0].message.contains("'{{' is not allowed"));
}

#[test]
fn interp_string_preserves_utf8() {
    let result = lex("`héllo {x} wörld`", LuaVersion::Luau);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    let TokenKind::InterpBegin(begin_text) = &result.tokens[0].kind else {
        panic!("expected InterpBegin, got {:?}", result.tokens[0].kind);
    };
    assert_eq!(begin_text.as_str(), "héllo ");
    let end = result
        .tokens
        .iter()
        .find_map(|token| match &token.kind {
            TokenKind::InterpEnd(text) => Some(text.as_str().to_string()),
            _ => None,
        })
        .expect("InterpEnd token");
    assert_eq!(end, " wörld");
}

#[test]
fn interp_string_unicode_escape_is_not_interpolation() {
    // `\u{41}` is a unicode escape; the `{` must not open an expression.
    let result = lex("`a\u{41}b`", LuaVersion::Luau);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    let TokenKind::InterpBegin(begin_text) = &result.tokens[0].kind else {
        panic!("expected InterpBegin, got {:?}", result.tokens[0].kind);
    };
    // Plain string form: InterpBegin("") + InterpEnd(full text).
    assert_eq!(begin_text.as_str(), "");
    let TokenKind::InterpEnd(end_text) = &result.tokens[1].kind else {
        panic!("expected InterpEnd, got {:?}", result.tokens[1].kind);
    };
    assert_eq!(end_text.as_str(), "a\u{41}b");
}

#[test]
fn interp_string_rejects_raw_newline() {
    let result = lex("`broken\nstring`", LuaVersion::Luau);
    assert!(!result.errors.is_empty(), "raw newline must be an error");
}

#[test]
fn bom_is_skipped() {
    let result = lex("\u{FEFF}local x = 1", LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert_eq!(result.tokens[0].kind, TokenKind::Local);
}

#[test]
fn bom_then_shebang_is_skipped() {
    let result = lex("\u{FEFF}#!/usr/bin/lua\nprint(1)", LuaVersion::Lua54);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    assert_eq!(result.tokens[0].kind, TokenKind::Identifier("print".into()));
}

#[test]
fn non_ascii_unexpected_char_is_one_error() {
    let result = lex("local café = 1", LuaVersion::Lua54);
    assert_eq!(result.errors.len(), 1, "errors: {:?}", result.errors);
    assert!(result.errors[0].message.contains('é'));
}
