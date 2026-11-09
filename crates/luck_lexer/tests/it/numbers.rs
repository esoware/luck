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
fn hex_number_adjacent_to_concat_is_malformed_outside_lua51() {
    // 5.1's hex scanner stops at the dot, so `0xFF..2` is a valid concat
    // there. Luau's consumes the dots, and 5.2+ fails in the hex-float path.
    assert_eq!(lex("return 0xFF..2", LuaVersion::Lua51).errors, Vec::<SourceError>::new());
    assert_ne!(lex("return 0xFF..2", LuaVersion::Luau).errors, Vec::<SourceError>::new());
    assert_ne!(lex("return 0xFF..2", LuaVersion::Lua54).errors, Vec::<SourceError>::new());
    assert_ne!(lex("return 0b1..2", LuaVersion::Luau).errors, Vec::<SourceError>::new());
}

#[test]
fn hex_literal_without_digits_is_error() {
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
fn hex_floats_lex_in_lua52_plus() {
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
fn hex_float_is_error_in_lua51_and_luau() {
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
fn binary_literal_lexes_only_in_luau() {
    assert_eq!(first_kind_in("0b1010", LuaVersion::Luau), TokenKind::Number("0b1010".into()));
    assert_eq!(first_kind_in("0B11", LuaVersion::Luau), TokenKind::Number("0B11".into()));
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert_malformed_number("0b1010", version);
    }
}

#[test]
fn binary_literal_without_digits_is_error() {
    let result = lex("0b", LuaVersion::Luau);
    assert_ne!(result.errors, Vec::<SourceError>::new());
    assert!(result.errors[0].message.contains("binary literal requires at least one digit"));
}

#[test]
fn underscore_separators_lex_only_in_luau() {
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

    // 5.1, 5.4, and 5.5 read a touching `_` as part of a malformed numeral.
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_malformed_number("1_000", version);
    }

    // 5.2 and 5.3 stop at a non-hex byte, leaving a number and a name.
    for version in [LuaVersion::Lua52, LuaVersion::Lua53] {
        assert_eq!(
            kinds_in("1_000", version),
            vec![TokenKind::Number("1".into()), TokenKind::Identifier("_000".into()),]
        );
    }
}

#[test]
fn integer_suffix_lexes_only_on_luau_integers_within_64_bits() {
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

    for source in ["2.5i", "1e5i", "1E5i", "1.i"] {
        assert!(
            !lex(source, LuaVersion::Luau).errors.is_empty(),
            "{source} must be a malformed integer"
        );
    }

    assert_malformed_number("1i", LuaVersion::Lua54);
    assert_malformed_number("1ix", LuaVersion::Luau);

    let lua53 = lex("1i", LuaVersion::Lua53);
    assert_eq!(lua53.tokens[0].kind, TokenKind::Number("1".into()));
    assert_eq!(lua53.tokens[1].kind, TokenKind::Identifier("i".into()));
}

fn assert_malformed_number(source: &str, version: LuaVersion) {
    let result = lex(source, version);
    assert!(
        result.errors.iter().any(|error| error.message.starts_with("malformed number")),
        "{source} under {version:?}: {:?}",
        result.errors
    );
}

#[test]
fn decimal_numeral_touching_a_letter_is_malformed_on_strict_versions() {
    for version in [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Lua55, LuaVersion::Luau] {
        for source in ["1then", "1a", "1g", "1.5x", "1e5x", "1.foo", "1.then", ".5x", "1z2"] {
            assert_malformed_number(source, version);
        }
    }
}

#[test]
fn decimal_numeral_touching_a_hex_digit_is_malformed_on_lua52_and_lua53() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53] {
        for source in ["1a", "1F", "1.f", "1.5e2a", ".5c"] {
            assert_malformed_number(source, version);
        }
    }
}

#[test]
fn decimal_numeral_splits_before_a_non_hex_letter_on_lua52_and_lua53() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53] {
        assert_eq!(
            kinds_in("1then", version),
            vec![TokenKind::Number("1".into()), TokenKind::Then]
        );
        assert_eq!(
            kinds_in("1.then", version),
            vec![TokenKind::Number("1.".into()), TokenKind::Then]
        );
        assert_eq!(
            kinds_in("1.5g", version),
            vec![TokenKind::Number("1.5".into()), TokenKind::Identifier("g".into())]
        );
        assert_eq!(
            kinds_in("1e5x", version),
            vec![TokenKind::Number("1e5".into()), TokenKind::Identifier("x".into())]
        );
    }
}

#[test]
fn malformed_decimal_numeral_error_spans_the_numeral_and_the_letter() {
    let result = lex("x = 12ab", LuaVersion::Lua54);
    assert_eq!(result.errors[0].span, Span::new(4, 7));
}

#[test]
fn decimal_numeral_before_a_separated_word_lexes_on_every_version() {
    for version in LuaVersion::ALL {
        assert_eq!(
            kinds_in("1 then", version),
            vec![TokenKind::Number("1".into()), TokenKind::Then],
            "{version:?}"
        );
    }
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

#[test]
fn bug_decimal_exponent_directly_before_concat_lexes_on_lua51_and_luau() {
    for version in [LuaVersion::Lua51, LuaVersion::Luau] {
        let result = lex("return 1e5..2", version);
        assert_eq!(
            result.errors,
            Vec::<SourceError>::new(),
            "`1e5..2` should lex as a concat under {version:?}, whose scanners stop at `.` after an exponent"
        );
    }
}

#[test]
fn bug_luau_hex_and_binary_numerals_directly_before_concat_lex() {
    for source in ["return 0xFF..2", "return 0b1..2"] {
        let result = lex(source, LuaVersion::Luau);
        assert_eq!(
            result.errors,
            Vec::<SourceError>::new(),
            "`{source}` should lex as a concat, because Luau's scanner stops at `.` after the radix prefix"
        );
    }
}

#[test]
fn bug_hex_numeral_splits_before_a_non_hex_letter_on_lua52_and_lua53() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53] {
        let result = lex("x=0x1g=2", version);
        assert_eq!(
            result.errors,
            Vec::<SourceError>::new(),
            "`0x1g` should split into `0x1` and `g` under {version:?}, as `x=0x1g=2` compiles there"
        );
        assert_eq!(result.tokens[2].kind, TokenKind::Number("0x1".into()));
        assert_eq!(result.tokens[3].kind, TokenKind::Identifier("g".into()));
    }
}

#[test]
fn dot_zero_x_is_malformed_on_lua52_and_lua53() {
    for version in [LuaVersion::Lua52, LuaVersion::Lua53] {
        let result = lex("x=.0x1=2", version);
        assert!(
            !result.errors.is_empty(),
            "`.0x1` should be a malformed number under {version:?}, not `.0` and `x1`: {:?}",
            result.tokens
        );
    }
}

#[test]
fn luau_binary_numeral_touching_a_letter_is_malformed() {
    let result = lex("x=0b1y=2", LuaVersion::Luau);
    assert!(
        !result.errors.is_empty(),
        "`0b1y` should be a malformed number in Luau, not `0b1` and `y`: {:?}",
        result.tokens
    );
}

#[test]
fn luau_binary_literal_of_only_underscores_is_error() {
    let result = lex("return 0b_", LuaVersion::Luau);
    assert!(
        !result.errors.is_empty(),
        "`0b_` has no digits, so it should be a malformed number: {:?}",
        result.tokens
    );
}

#[test]
fn luau_exponent_of_only_underscores_is_error() {
    for source in ["return 1e_", "return 1e+_"] {
        let result = lex(source, LuaVersion::Luau);
        assert!(
            !result.errors.is_empty(),
            "`{source}` has no exponent digits, so it should be a malformed number: {:?}",
            result.tokens
        );
    }
}

#[test]
fn luau_underscore_after_integer_suffix_lexes() {
    let result = lex("return 1i_", LuaVersion::Luau);
    assert_eq!(
        result.errors,
        Vec::<SourceError>::new(),
        "Luau strips every `_` before conversion, so `1i_` should be the integer `1i`"
    );
}

#[test]
fn luau_underscore_inside_radix_prefix_lexes() {
    for source in ["return 0_x1", "return 0_b1"] {
        let result = lex(source, LuaVersion::Luau);
        assert_eq!(
            result.errors,
            Vec::<SourceError>::new(),
            "Luau strips every `_` before conversion, so `{source}` should keep its radix prefix"
        );
    }
}
