use luck_lexer::lex;
use luck_token::{LuaVersion, SourceError, Span, TokenKind};

use crate::common::{first_kind, first_kind_in, kinds_in};

#[test]
fn numerals_keep_their_spelling() {
    for source in
        ["42", "3.14", ".5", "1e10", "3.14e-2", "1E+5", "0xFF", "0XAB", "1.", "1.e5", "0."]
    {
        assert_eq!(first_kind(source), TokenKind::Number(source.into()));
    }
}

#[test]
fn decimal_float_lexes_in_every_version() {
    for version in LuaVersion::ALL {
        let result = lex("3.14e2", version);
        assert!(result.errors.is_empty(), "3.14e2 should work in {version:?}");
    }
}

#[test]
fn hex_integer_lexes_in_every_version() {
    for version in LuaVersion::ALL {
        let result = lex("0xFF", version);
        assert!(result.errors.is_empty(), "0xFF should work in {version:?}");
        assert_eq!(result.tokens[0].kind, TokenKind::Number("0xFF".into()));
    }
}

#[test]
fn number_adjacent_to_concat_is_malformed() {
    // Every real Lua numeral scanner consumes the dots, so `1..2` is
    // "malformed number", never `1 .. 2`.
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau] {
        for source in ["return 1..2", "return 1.5..2", "return 1e5..2"] {
            let result = lex(source, version);
            assert!(!result.errors.is_empty(), "{source} must be malformed under {version:?}");
        }
    }
    // With a space the concat is valid.
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Luau] {
        let result = lex("return 1 ..2", version);
        assert!(result.errors.is_empty(), "spaced concat lexes: {version:?}");
    }
}

#[test]
fn hex_number_adjacent_to_concat_per_version() {
    // 5.1's hex scanner stops at the dot, so `0xFF..2` is a valid concat
    // there. Luau's consumes the dots, and 5.2+ fails in the hex-float path.
    assert_eq!(lex("return 0xFF..2", LuaVersion::Lua51).errors, Vec::<SourceError>::new());
    assert_ne!(lex("return 0xFF..2", LuaVersion::Luau).errors, Vec::<SourceError>::new());
    assert_ne!(lex("return 0xFF..2", LuaVersion::Lua54).errors, Vec::<SourceError>::new());
    assert_ne!(lex("return 0b1..2", LuaVersion::Luau).errors, Vec::<SourceError>::new());
}

#[test]
fn hex_no_digits_is_error() {
    let result = lex("0x", LuaVersion::Lua54);
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("hex literal requires at least one digit"));
}

#[test]
fn digitless_hex_prefix_is_error_in_every_version() {
    // A `.` after `0x` opens a fraction only where hex floats exist; the
    // versions without them must still reject the digitless prefix rather
    // than emitting `0x` as a number and leaving the dots to lex as concat.
    for version in LuaVersion::ALL {
        for source in ["0x", "0x.", "0x..1"] {
            assert!(!lex(source, version).errors.is_empty(), "{version:?} must reject {source}");
        }
    }
    // Underscores separate digits but aren't digits themselves.
    assert_ne!(lex("0x_", LuaVersion::Luau).errors, Vec::<SourceError>::new());
}

#[test]
fn hex_floats_lex_in_every_version_that_has_them() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for source in ["0x1.Fp10", "0xAp4", "0xA23p-4", "0x1.0p+3"] {
            assert_eq!(
                first_kind_in(source, version),
                TokenKind::Number(source.into()),
                "failed for: {source} in {version:?}"
            );
        }
    }
}

#[test]
fn hex_float_rejected_in_unsupported_versions() {
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        let result = lex("0x1.Fp10", version);
        assert!(!result.errors.is_empty(), "hex float should error in {version:?}");
    }
}

#[test]
fn hex_float_exponent_is_optional() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for source in ["0x.1", "0X0.41", "0xA.8", "0x1.F", "0x1.", "0x1.p2", "0x.1p-2"] {
            let result = lex(source, version);
            assert!(result.errors.is_empty(), "{version:?} {source}: {:?}", result.errors);
            assert_eq!(result.tokens[0].kind, TokenKind::Number(source.into()));
        }
        for source in ["0x.", "0xp2", "0x1.p", "0x.1p+", "0x1..2"] {
            let result = lex(source, version);
            assert!(!result.errors.is_empty(), "{version:?} {source}");
        }
    }
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        for source in ["0x.1", "0X0.41", "0xA.8"] {
            assert!(!lex(source, version).errors.is_empty(), "{version:?} {source}");
        }
    }
}

#[test]
fn hex_numeral_touching_a_letter_is_error() {
    // The optional exponent must not turn `0xFF.foo` into the number
    // `0xFF.f` followed by the name `oo`.
    for version in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for source in ["0xFF.foo", "0x1.5g", "0x1.5p2x", "0x1_2"] {
            assert!(!lex(source, version).errors.is_empty(), "{version:?} must reject {source}");
        }
    }
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        assert!(!lex("0xFFz", version).errors.is_empty(), "{version:?} must reject 0xFFz");
    }
}

#[test]
fn binary_literal_luau_only() {
    assert_eq!(first_kind_in("0b1010", LuaVersion::Luau), TokenKind::Number("0b1010".into()));
    assert_eq!(first_kind_in("0B11", LuaVersion::Luau), TokenKind::Number("0B11".into()));
    // Outside Luau, `0b1010` splits into the number `0` and the name `b1010`.
    let token_kinds = kinds_in("0b1010", LuaVersion::Lua54);
    assert_eq!(token_kinds[0], TokenKind::Number("0".into()));
    assert_eq!(token_kinds[1], TokenKind::Identifier("b1010".into()));
}

#[test]
fn binary_no_digits_is_error() {
    let result = lex("0b", LuaVersion::Luau);
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("binary literal requires at least one digit"));
}

#[test]
fn underscore_separators_luau_only() {
    for source in ["1_000_000", "0xFF_FF", "0b1010_0101", "0b1111_0000", "1_0e1_0", "0xDEAD_BEEF"] {
        assert_eq!(
            first_kind_in(source, LuaVersion::Luau),
            TokenKind::Number(source.into()),
            "failed for: {source}"
        );
    }

    // Real Luau strips underscores anywhere before conversion, so
    // prefix-adjacent, doubled, and trailing underscores all lex.
    for source in ["0x_FF", "0b_0101_0101", "100_", "1__2", "0xFF_"] {
        assert!(lex(source, LuaVersion::Luau).errors.is_empty(), "{source} must lex under Luau");
    }

    // Outside Luau, it splits into a number and a name.
    assert_eq!(
        kinds_in("1_000", LuaVersion::Lua54),
        vec![TokenKind::Number("1".into()), TokenKind::Identifier("_000".into()),]
    );
}

#[test]
fn luau_integer_literal_suffix_and_range() {
    for source in [
        "1i",
        "1_000_000i",
        "0xabi",
        "0XAB05i",
        "0xff_ffi",
        "0xffffffffffffffffi",
        "0b1111111111111111111111111111111111111111111111111111111111111111i",
    ] {
        let result = lex(source, LuaVersion::Luau);
        assert!(result.errors.is_empty(), "{source}: {:?}", result.errors);
        assert_eq!(result.tokens[0].kind, TokenKind::Number(source.into()));
    }

    // Decimal overflow applies even under a direct unary minus;
    // i64::MIN is only spellable as 0x8000000000000000i.
    for source in [
        "9223372036854775808i",
        "0x10000000000000000i",
        "0b10000000000000000000000000000000000000000000000000000000000000000i",
    ] {
        assert!(!lex(source, LuaVersion::Luau).errors.is_empty(), "{source} must overflow");
    }

    // `1.i` isn't listed: `.` followed by a letter ends the number, so it
    // lexes as field access on `1`.
    for source in ["2.5i", "1e5i", "1E5i"] {
        assert!(
            !lex(source, LuaVersion::Luau).errors.is_empty(),
            "{source} must be a malformed integer"
        );
    }

    let non_luau = lex("1i", LuaVersion::Lua54);
    assert_eq!(non_luau.tokens[0].kind, TokenKind::Number("1".into()));
    assert_eq!(non_luau.tokens[1].kind, TokenKind::Identifier("i".into()));
}

#[test]
fn integer_range_normalization_preserves_boundaries_and_spans() {
    for (digits, radix, is_valid) in [
        ("9223372036854775807", "", true),
        ("9223372036854775808", "", false),
        ("FFFFFFFFFFFFFFFF", "0X", true),
        ("10000000000000000", "0x", false),
        ("1111111111111111111111111111111111111111111111111111111111111111", "0B", true),
        ("10000000000000000000000000000000000000000000000000000000000000000", "0b", false),
    ] {
        let separated = digits.chars().flat_map(|digit| [digit, '_']).collect::<String>();
        for spelling in [digits, separated.as_str()] {
            let literal = format!("{radix}{spelling}i");
            let source = format!("  {literal}");
            let result = lex(&source, LuaVersion::Luau);
            assert_eq!(result.errors.is_empty(), is_valid, "{source}: {:?}", result.errors);
            let span = Span::from_indices(2, source.len());
            if is_valid {
                assert_eq!(result.tokens[0].kind, TokenKind::Number(literal.into()));
                assert_eq!(result.tokens[0].span, span);
            } else {
                assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
                assert_eq!(result.errors[0].span, span);
                assert_eq!(result.errors[0].message, "integer literal is outside the 64-bit range");
            }
        }
    }
}
