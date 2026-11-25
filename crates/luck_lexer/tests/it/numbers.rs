use luck_diagnostics::Diagnostic;
use luck_lexer::lex;
use luck_token::{Span, TokenKind};

use crate::common::{token_texts, tokens};

fn assert_malformed_number(source: &str) {
    let result = lex(source.as_bytes());
    assert!(
        result.errors.iter().any(|e| e.message.starts_with("malformed number")),
        "{source}: {:?}",
        result.errors
    );
}

#[test]
fn numerals_keep_their_spelling() {
    for source in
        ["42", "3.14", ".5", "1e10", "3.14e-2", "1E+5", "0xFF", "0XAB", "1.", "1.e5", "0."]
    {
        assert_eq!(tokens(source), [(TokenKind::Number, source)]);
    }
}

#[test]
fn number_adjacent_to_concat_is_malformed() {
    // The scanner takes every dot short of an exponent, so `1..2` is
    // "malformed number", never `1 .. 2`.
    for source in ["return 1..2", "return 1.5..2"] {
        let result = lex(source.as_bytes());
        assert!(!result.errors.is_empty(), "{source} must be malformed");
    }
    // With a space the concat is valid.
    assert_eq!(lex(b"return 1 ..2").errors, Vec::<Diagnostic>::new());
}

#[test]
fn a_numeral_directly_before_concat_lexes_after_an_exponent_or_a_radix_prefix() {
    for source in ["return 1e5..2", "return 0xFF..2", "return 0b1..2"] {
        let result = lex(source.as_bytes());
        assert_eq!(
            result.errors,
            Vec::<Diagnostic>::new(),
            "`{source}` should lex as a concat, because Luau's scanner stops at `.` there"
        );
    }
}

#[test]
fn hex_literal_without_digits_is_error() {
    for source in ["0x", "0x.", "0x..1", "0x_"] {
        let result = lex(source.as_bytes());
        assert_ne!(result.errors, Vec::<Diagnostic>::new(), "{source}");
        assert!(
            result.errors[0].message.contains("hex literal requires at least one digit"),
            "{source}: {:?}",
            result.errors
        );
    }
}

#[test]
fn hex_floats_are_errors() {
    for source in ["0x1.Fp10", "0xAp4", "0x.1", "0X0.41", "0xA.8", "0x1p-2"] {
        assert!(!lex(source.as_bytes()).errors.is_empty(), "{source}");
    }
}

#[test]
fn hex_numeral_touching_a_letter_is_malformed() {
    for source in ["0xFFz", "0x1g", "0x1_2g"] {
        assert!(!lex(source.as_bytes()).errors.is_empty(), "must reject {source}");
    }
}

#[test]
fn binary_literals_keep_their_spelling() {
    assert_eq!(tokens("0b1010"), [(TokenKind::Number, "0b1010")]);
    assert_eq!(tokens("0B11"), [(TokenKind::Number, "0B11")]);
}

#[test]
fn binary_literal_without_digits_is_error() {
    for source in ["0b", "return 0b_"] {
        let result = lex(source.as_bytes());
        assert_ne!(result.errors, Vec::<Diagnostic>::new(), "{source}");
        assert!(result.errors[0].message.contains("binary literal requires at least one digit"));
    }
}

#[test]
fn underscore_separators_lex_anywhere() {
    for source in [
        "1_000_000",
        "0xFF_FF",
        "0b1010_0101",
        "1_0e1_0",
        "0xDEAD_BEEF",
        "0x_FF",
        "0b_0101_0101",
        "100_",
        "1__2",
        "0xFF_",
    ] {
        assert_eq!(tokens(source), [(TokenKind::Number, source)], "failed for: {source}");
    }
}

#[test]
fn an_underscore_inside_a_radix_prefix_keeps_it() {
    for source in ["0_x1", "0_b1"] {
        assert_eq!(tokens(source), [(TokenKind::Number, source)], "{source}");
    }
}

#[test]
fn integer_suffix_lexes_only_on_integers_within_64_bits() {
    for source in [
        "1i",
        "1_000_000i",
        "0xabi",
        "0XAB05i",
        "0xff_ffi",
        "0xffffffffffffffffi",
        "0b1111111111111111111111111111111111111111111111111111111111111111i",
        "1i_",
    ] {
        assert_eq!(tokens(source), [(TokenKind::Integer, source)]);
    }

    // Decimal overflow applies even under a direct unary minus;
    // i64::MIN is only spellable as 0x8000000000000000i.
    for source in [
        "9223372036854775808i",
        "0x10000000000000000i",
        "0b10000000000000000000000000000000000000000000000000000000000000000i",
    ] {
        assert!(!lex(source.as_bytes()).errors.is_empty(), "{source} must overflow");
    }

    for source in ["2.5i", "1e5i", "1E5i", "1.i"] {
        assert!(!lex(source.as_bytes()).errors.is_empty(), "{source} must be a malformed integer");
    }

    assert_malformed_number("1ix");
}

#[test]
fn a_numeral_touching_a_letter_is_malformed() {
    for source in
        ["1then", "1a", "1g", "1.5x", "1e5x", "1.foo", "1.then", ".5x", "1z2", ".0x1", "0b1y"]
    {
        assert_malformed_number(source);
    }
}

#[test]
fn malformed_decimal_numeral_error_spans_the_numeral_and_the_letter() {
    let result = lex(b"x = 12ab");
    assert_eq!(result.errors[0].span, Some(Span::new(4, 7)));
}

#[test]
fn decimal_numeral_before_a_separated_word_lexes() {
    assert_eq!(tokens("1 then"), vec![(TokenKind::Number, "1"), (TokenKind::Then, "then")]);
}

#[test]
fn an_exponent_needs_a_digit_past_its_underscores() {
    for source in ["return 1e", "return 1e_", "return 1e+_"] {
        let result = lex(source.as_bytes());
        assert!(
            !result.errors.is_empty(),
            "`{source}` has no exponent digits, so it should be a malformed number: {:?}",
            result.tokens
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
            let result = lex(source.as_bytes());
            assert_eq!(result.errors.is_empty(), is_valid, "{source}: {:?}", result.errors);
            let span = Span::from_indices(2, source.len());
            if is_valid {
                assert_eq!(token_texts(&result, &source), [(TokenKind::Integer, literal.as_str())]);
                assert_eq!(result.tokens[0].span, span);
            } else {
                assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
                assert_eq!(result.errors[0].span, Some(span));
                assert_eq!(result.errors[0].message, "integer literal is outside the 64-bit range");
            }
        }
    }
}
