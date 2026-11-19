use core::iter;

use luck_token::{
    LuaNumber, LuaVersion, Numeral, read_integer, read_numeral, spell_integer, spell_number,
};

fn value(text: &str, version: LuaVersion) -> LuaNumber {
    read_numeral(text, version).value
}

fn is_exact(text: &str, version: LuaVersion) -> bool {
    read_numeral(text, version).is_exact
}

fn written(number: LuaNumber, version: LuaVersion) -> String {
    spell_number(number, version).as_str().to_owned()
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
    mixed
        .take(4000)
        .enumerate()
        .map(|(i, bits)| bits >> u32::try_from(i.rem_euclid(64)).unwrap_or_default())
}

#[test]
fn integers_split_from_floats_only_from_lua53() {
    for version in LuaVersion::ALL {
        let expected =
            if version.has_integer_subtype() { LuaNumber::Int(42) } else { LuaNumber::Float(42.0) };
        assert_eq!(value("42", version), expected, "{version:?}");
        assert_eq!(value("1.5", version), LuaNumber::Float(1.5), "{version:?}");
        assert_eq!(value("1e3", version), LuaNumber::Float(1000.0), "{version:?}");
    }
    assert_ne!(value("1", LuaVersion::Lua54), value("1.0", LuaVersion::Lua54));
    assert_eq!(value("1", LuaVersion::Lua51), value("1.0", LuaVersion::Lua51));
}

#[test]
fn equality_is_identity() {
    assert_ne!(LuaNumber::Int(1), LuaNumber::Float(1.0));
    assert_ne!(LuaNumber::Float(0.0f64), LuaNumber::Float(-0.0f64));
    assert_eq!(LuaNumber::Float(f64::NAN), LuaNumber::Float(f64::NAN));
    assert_eq!(LuaNumber::Int(-3).to_f64(), -3.0f64);
}

#[test]
fn decimals_read_every_spelling_the_lexer_takes() {
    for (text, expected) in [
        ("0.5", 0.5f64),
        (".5", 0.5f64),
        ("5.", 5.0f64),
        ("1E3", 1000.0f64),
        ("1.5e-1", 0.15f64),
        ("2e+2", 200.0f64),
        ("007.5", 7.5f64),
        ("1e999", f64::INFINITY),
    ] {
        assert_eq!(value(text, LuaVersion::Lua54), LuaNumber::Float(expected), "{text}");
    }
}

#[test]
fn a_decimal_past_i64_max_is_a_float_on_lua53_and_rounds_elsewhere() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            read_numeral("9223372036854775807", version),
            Numeral { value: LuaNumber::Int(i64::MAX), is_exact: true }
        );
        assert_eq!(
            read_numeral("9223372036854775808", version),
            Numeral { value: LuaNumber::Float(2.0f64.powi(63)), is_exact: false }
        );
    }
    assert!(!is_exact("9007199254740993", LuaVersion::Lua51));
    assert!(is_exact("9007199254740994", LuaVersion::Lua51));
    assert!(is_exact("1.5", LuaVersion::Lua51), "a float spelling counts as exact");
}

#[test]
fn hex_integers_wrap_on_lua53_and_round_elsewhere() {
    let lua54 = LuaVersion::Lua54;
    assert_eq!(value("0xFF", lua54), LuaNumber::Int(255));
    assert_eq!(value("0XdEaD", lua54), LuaNumber::Int(0xDEAD));
    assert_eq!(value("0xffffffffffffffff", lua54), LuaNumber::Int(-1));
    assert_eq!(value("0x8000000000000000", lua54), LuaNumber::Int(i64::MIN));
    assert_eq!(read_numeral("0x10000000000000001", lua54).value, LuaNumber::Int(1));
    assert!(!is_exact("0x10000000000000001", lua54));
    assert!(is_exact("0x0000000000000000000ff", lua54), "leading zeros don't count");

    for version in [LuaVersion::Lua51, LuaVersion::Lua52] {
        assert_eq!(value("0xFF", version), LuaNumber::Float(255.0));
        assert_eq!(value("0x10000000000000000", version), LuaNumber::Float(2.0f64.powi(64)));
        assert!(is_exact("0x10000000000000000", version), "2^64 is a double");
        assert!(!is_exact("0x20000000000001", version), "2^53 + 1 is not");
        assert_eq!(
            value("0x20000000000003", version),
            LuaNumber::Float(9_007_199_254_740_996.0f64)
        );
    }
}

#[test]
fn luau_saturates_hex_and_binary_numerals_past_64_bits() {
    let luau = LuaVersion::Luau;
    let two_to_64 = LuaNumber::Float(2.0f64.powi(64));
    assert_eq!(
        read_numeral("0x10000000000000000", luau),
        Numeral { value: two_to_64, is_exact: false }
    );
    let binary = format!("0b1{}", "0".repeat(64));
    assert_eq!(read_numeral(&binary, luau), Numeral { value: two_to_64, is_exact: false });
    assert_eq!(value("0b101", luau), LuaNumber::Float(5.0));
    assert_eq!(value("0B_1_0", luau), LuaNumber::Float(2.0));
}

#[test]
fn luau_drops_separators_anywhere() {
    let luau = LuaVersion::Luau;
    for (text, expected) in [
        ("1_000", 1000.0f64),
        ("1__2", 12.0f64),
        ("12_", 12.0f64),
        ("0_x1_0", 16.0f64),
        ("1_0.5_0", 10.5f64),
    ] {
        assert_eq!(value(text, luau), LuaNumber::Float(expected), "{text}");
    }
    let long = format!("1{}", "_0".repeat(40));
    assert_eq!(value(&long, luau), LuaNumber::Float(1e40f64));
}

#[test]
fn hex_floats_read_as_the_nearest_double() {
    let lua54 = LuaVersion::Lua54;
    for (text, expected) in [
        ("0x1.8p1", 3.0f64),
        ("0x.1p4", 1.0f64),
        ("0X1P-1", 0.5f64),
        ("0xA.8", 10.5f64),
        ("0x1p-1074", f64::from_bits(1)),
        ("0x1p-1075", 0.0f64),
        ("0x1.000001p-1075", f64::from_bits(1)),
        ("0x3p-1075", f64::from_bits(2)),
        ("0x1.fffffffffffffp1023", f64::MAX),
        ("0x1.fffffffffffff8p1023", f64::INFINITY),
        ("0x1p1024", f64::INFINITY),
        ("0x1p-99999999999999", 0.0f64),
        ("0x0.0p0", 0.0f64),
        ("0x1.00000000000008p0", 1.0f64),
        ("0x1.000000000000081p0", 1.000_000_000_000_000_2f64),
    ] {
        assert_eq!(value(text, lua54), LuaNumber::Float(expected), "{text}");
    }
}

#[test]
fn long_hex_numerals_round_once() {
    // 5^16, so `x / 2^16` is `x * 5^16 / 10^16`.
    let five_to_16 = 152_587_890_625u128;
    for bits in samples() {
        let wide = u128::from(bits);
        let hex = |text: String, version| value(&text, version);
        assert_eq!(hex(format!("0x{bits:x}"), LuaVersion::Lua52), nearest_to(wide, 0));
        let sixteen_times = nearest_to(wide << 4u32, 0);
        assert_eq!(hex(format!("0x{bits:x}0"), LuaVersion::Lua51), sixteen_times, "{bits:x}");
        let fraction = nearest_to(wide.wrapping_mul(five_to_16), 16);
        assert_eq!(hex(format!("0x{bits:x}p-16"), LuaVersion::Lua54), fraction, "{bits:x}");
    }
}

/// The double nearest to `numerator / 10^decimals`, as Rust's correctly
/// rounding parser reads it.
fn nearest_to(numerator: u128, decimals: u32) -> LuaNumber {
    LuaNumber::Float(format!("{numerator}e-{decimals}").parse().unwrap_or(f64::NAN))
}

#[test]
fn luau_integer_literals_read_64_bit_patterns() {
    assert_eq!(read_integer("123i"), Some(123));
    assert_eq!(read_integer("1_000i__"), Some(1000));
    assert_eq!(read_integer("0xffffffffffffffffi"), Some(-1));
    assert_eq!(read_integer("0b101i"), Some(5));
    assert_eq!(read_integer("9223372036854775807i"), Some(i64::MAX));
    assert_eq!(read_integer("9223372036854775808i"), None);
    assert_eq!(read_integer("0x10000000000000000i"), None);
}

#[test]
fn writes_the_shortest_numeral_for_each_dialect() {
    let (lua54, luau) = (LuaVersion::Lua54, LuaVersion::Luau);
    for (number, version, expected) in [
        (LuaNumber::Int(0), lua54, "0"),
        (LuaNumber::Int(255), lua54, "255"),
        (LuaNumber::Int(-1), lua54, "0xffffffffffffffff"),
        (LuaNumber::Int(i64::MIN), lua54, "0x8000000000000000"),
        (LuaNumber::Int(999_999_999_999_999), lua54, "999999999999999"),
        (LuaNumber::Int(1_000_000_000_000_000_000), lua54, "0xde0b6b3a7640000"),
        (LuaNumber::Float(0.0), lua54, "0."),
        (LuaNumber::Float(0.0), luau, "0"),
        (LuaNumber::Float(3.0), lua54, "3."),
        (LuaNumber::Float(3.0), luau, "3"),
        (LuaNumber::Float(0.5), lua54, ".5"),
        (LuaNumber::Float(1.5), lua54, "1.5"),
        (LuaNumber::Float(100.0), lua54, "1e2"),
        (LuaNumber::Float(100.0), luau, "100"),
        (LuaNumber::Float(1000.0), luau, "1e3"),
        (LuaNumber::Float(1_500_000.0), lua54, "15e5"),
        (LuaNumber::Float(0.0001), lua54, "1e-4"),
        (LuaNumber::Float(0.000_010_012), lua54, "10012e-9"),
        (LuaNumber::Float(1e300), lua54, "1e300"),
        (LuaNumber::Float(f64::INFINITY), luau, "1e999"),
        (LuaNumber::Float(f64::from_bits(1)), lua54, "5e-324"),
        (LuaNumber::Float(4_503_599_627_370_495.0), luau, "0xfffffffffffff"),
    ] {
        assert_eq!(written(number, version), expected, "{number:?} {version:?}");
    }
    assert_eq!(spell_integer(-1).as_str(), "0xffffffffffffffffi");
    assert_eq!(spell_integer(42).as_str(), "42i");
}

#[test]
fn every_written_number_reads_back_as_itself() {
    for bits in samples() {
        let float = f64::from_bits(bits >> 1u32);
        let numbers = [LuaNumber::Int(bits.cast_signed()), LuaNumber::Float(float)];
        for number in numbers {
            if matches!(number, LuaNumber::Float(nan) if nan.is_nan()) {
                continue;
            }
            for version in LuaVersion::ALL {
                if matches!(number, LuaNumber::Int(_)) && !version.has_integer_subtype() {
                    continue;
                }
                let text = written(number, version);
                assert_eq!(value(&text, version), number, "{text} {version:?}");
                assert_eq!(written(value(&text, version), version), text, "{version:?}");
                if let LuaNumber::Float(written_float) = number {
                    let shortest =
                        format!("{written_float:e}").len().min(format!("{written_float}").len());
                    assert!(text.len() <= shortest.saturating_add(1), "{text} {written_float:e}");
                }
            }
        }
    }
}
