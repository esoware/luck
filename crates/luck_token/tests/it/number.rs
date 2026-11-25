use std::iter;

use luck_token::{Numeral, read_numeral, spell_number};

fn value(text: &str) -> f64 {
    read_numeral(text).value
}

fn is_exact(text: &str) -> bool {
    read_numeral(text).is_exact
}

fn written(number: f64) -> String {
    spell_number(number).as_str().to_owned()
}

/// A run of 64-bit values that covers every bit position, for the property
/// tests.
fn samples() -> impl Iterator<Item = u64> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mixed = iter::repeat_with(move || {
        state =
            state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        state
    });
    mixed.take(4000).enumerate().map(|(i, bits)| bits >> (i % 64))
}

#[test]
fn every_numeral_reads_as_a_double() {
    assert_eq!(value("42"), 42.0);
    assert_eq!(value("1.5"), 1.5);
    assert_eq!(value("1e3"), 1000.0);
    assert_eq!(value("1"), value("1.0"));
}

#[test]
fn decimals_read_every_spelling_the_lexer_takes() {
    for (text, expected) in [
        ("0.5", 0.5),
        (".5", 0.5),
        ("5.", 5.0),
        ("1E3", 1000.0),
        ("1.5e-1", 0.15),
        ("2e+2", 200.0),
        ("007.5", 7.5),
        ("1e999", f64::INFINITY),
    ] {
        assert_eq!(value(text), expected, "{text}");
    }
}

#[test]
fn a_decimal_integer_is_exact_only_where_a_double_holds_it() {
    assert!(!is_exact("9007199254740993"));
    assert!(is_exact("9007199254740994"));
    assert!(is_exact("1.5"), "a float spelling counts as exact");
    assert_eq!(
        read_numeral("9223372036854775808"),
        Numeral { value: 2.0f64.powi(63), is_exact: true }
    );
}

#[test]
fn hex_numerals_round_to_the_nearest_double() {
    assert_eq!(value("0xFF"), 255.0);
    assert_eq!(value("0XdEaD"), 57_005.0);
    assert!(!is_exact("0x20000000000001"), "2^53 + 1 is not a double");
    assert_eq!(value("0x20000000000003"), 9_007_199_254_740_996.0);
    assert!(is_exact("0x0000000000000000000ff"), "leading zeros don't count");
}

#[test]
fn hex_and_binary_numerals_saturate_past_64_bits() {
    let two_to_64 = 2.0f64.powi(64);
    assert_eq!(read_numeral("0x10000000000000000"), Numeral { value: two_to_64, is_exact: false });
    let binary = format!("0b1{}", "0".repeat(64));
    assert_eq!(read_numeral(&binary), Numeral { value: two_to_64, is_exact: false });
    assert_eq!(value("0b101"), 5.0);
    assert_eq!(value("0B_1_0"), 2.0);
}

#[test]
fn separators_drop_anywhere() {
    for (text, expected) in
        [("1_000", 1000.0), ("1__2", 12.0), ("12_", 12.0), ("0_x1_0", 16.0), ("1_0.5_0", 10.5)]
    {
        assert_eq!(value(text), expected, "{text}");
    }
    let long = format!("1{}", "_0".repeat(40));
    assert_eq!(value(&long), 1e40);
}

#[test]
fn long_hex_numerals_round_once() {
    for bits in samples() {
        let wide = u128::from(bits);
        assert_eq!(value(&format!("0x{bits:x}")), nearest_to(wide), "{bits:x}");
        if bits >> 60 == 0 {
            assert_eq!(value(&format!("0x{bits:x}0")), nearest_to(wide << 4), "{bits:x}");
        }
    }
}

/// The double nearest to `integer`, as Rust's correctly rounding parser
/// reads it.
fn nearest_to(integer: u128) -> f64 {
    integer.to_string().parse().expect("a decimal numeral")
}

#[test]
fn writes_the_shortest_numeral() {
    for (number, expected) in [
        (0.0, "0"),
        (3.0, "3"),
        (255.0, "255"),
        (0.5, ".5"),
        (1.5, "1.5"),
        (100.0, "100"),
        (1000.0, "1e3"),
        (1_500_000.0, "15e5"),
        (0.0001, "1e-4"),
        (0.000_010_012, "10012e-9"),
        (1e300, "1e300"),
        (f64::INFINITY, "1e999"),
        (f64::from_bits(1), "5e-324"),
        (999_999_999_999_999.0, "999999999999999"),
        (4_503_599_627_370_495.0, "0xfffffffffffff"),
    ] {
        assert_eq!(written(number), expected, "{number:?}");
    }
}

#[test]
fn every_written_number_reads_back_as_itself() {
    for bits in samples() {
        let number = f64::from_bits(bits >> 1);
        if number.is_nan() {
            continue;
        }
        let text = written(number);
        assert_eq!(value(&text).to_bits(), number.to_bits(), "{text}");
        assert_eq!(written(value(&text)), text);
        let shortest = format!("{number:e}").len().min(format!("{number}").len());
        assert!(text.len() <= shortest + 1, "{text} {number:e}");
    }
}
