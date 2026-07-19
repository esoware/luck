//! Numeric-literal normalization.
//!
//! The base prefix (`0x`/`0b`) and exponent markers (`e`/`p`) are always
//! lowered - canonical spelling, not a style choice. Hex digit case follows
//! [`HexCase`]. Digit separators (`_`, Luau) and the numeric value are never
//! touched, so the result re-parses to the same number.

use crate::HexCase;

/// Return the normalized spelling of a numeric literal, or the input
/// unchanged when nothing applies (plain decimal in `Preserve` mode).
pub(crate) fn normalize_number(text: &str, hex_case: HexCase) -> String {
    let bytes = text.as_bytes();
    let is_hex = bytes.len() >= 2 && bytes[0] == b'0' && matches!(bytes[1], b'x' | b'X');
    let is_binary = bytes.len() >= 2 && bytes[0] == b'0' && matches!(bytes[1], b'b' | b'B');

    if !is_hex && !is_binary {
        return normalize_decimal(text);
    }

    let mut result = String::with_capacity(text.len());
    result.push('0');
    // Prefix letter is always lowercase.
    result.push(bytes[1].to_ascii_lowercase() as char);

    for &byte in &bytes[2..] {
        let ch = byte as char;
        match byte {
            // Hex-float binary exponent - marker lowercased, its sign/digits
            // (decimal) fall through unchanged.
            b'p' | b'P' if is_hex => result.push('p'),
            b'a'..=b'f' | b'A'..=b'F' if is_hex => result.push(apply_hex_case(ch, hex_case)),
            _ => result.push(ch),
        }
    }
    result
}

/// Decimal literals: the exponent marker `E`/`e` is always lowered, and a
/// bare leading dot gains a zero (`.5` -> `0.5`). Hex floats never reach
/// here - they always start with `0x`.
fn normalize_decimal(text: &str) -> String {
    let needs_leading_zero = text.starts_with('.');
    if !needs_leading_zero && !text.bytes().any(|byte| byte == b'E') {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len() + 1);
    if needs_leading_zero {
        result.push('0');
    }
    result.extend(text.chars().map(|ch| if ch == 'E' { 'e' } else { ch }));
    result
}

fn apply_hex_case(ch: char, hex_case: HexCase) -> char {
    match hex_case {
        HexCase::Preserve => ch,
        HexCase::Lower => ch.to_ascii_lowercase(),
        HexCase::Upper => ch.to_ascii_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_always_lowered() {
        assert_eq!(normalize_number("0XB0", HexCase::Preserve), "0xB0");
        assert_eq!(normalize_number("0Xb0", HexCase::Preserve), "0xb0");
        assert_eq!(normalize_number("0B1010", HexCase::Preserve), "0b1010");
    }

    #[test]
    fn digits_follow_config() {
        assert_eq!(normalize_number("0xDEad", HexCase::Preserve), "0xDEad");
        assert_eq!(normalize_number("0xDEad", HexCase::Lower), "0xdead");
        assert_eq!(normalize_number("0xDEad", HexCase::Upper), "0xDEAD");
    }

    #[test]
    fn decimal_exponent_lowered() {
        assert_eq!(normalize_number("1E10", HexCase::Preserve), "1e10");
        assert_eq!(normalize_number("1.5E-3", HexCase::Upper), "1.5e-3");
        // A plain integer is returned unchanged.
        assert_eq!(normalize_number("42", HexCase::Upper), "42");
    }

    #[test]
    fn hex_float_exponent_lowered_digits_cased() {
        // `p` marker lowered; `A` digit follows config; `-2` exponent intact.
        assert_eq!(normalize_number("0xAP-2", HexCase::Lower), "0xap-2");
        assert_eq!(normalize_number("0x1.8P3", HexCase::Preserve), "0x1.8p3");
    }

    #[test]
    fn separators_preserved() {
        assert_eq!(normalize_number("0xFF_FF", HexCase::Lower), "0xff_ff");
        assert_eq!(normalize_number("1_000_000", HexCase::Upper), "1_000_000");
    }

    #[test]
    fn leading_dot_gains_zero() {
        assert_eq!(normalize_number(".5", HexCase::Preserve), "0.5");
        assert_eq!(normalize_number(".5E2", HexCase::Preserve), "0.5e2");
        assert_eq!(normalize_number(".5e2", HexCase::Preserve), "0.5e2");
        // Already-zeroed forms and hex floats are untouched.
        assert_eq!(normalize_number("0.5", HexCase::Preserve), "0.5");
        assert_eq!(normalize_number("0x.8p1", HexCase::Preserve), "0x.8p1");
    }

    #[test]
    fn idempotent() {
        for input in ["0XB0", "0xDEad", "1E10", "0xAP-2", "0B10", ".5", ".5E2"] {
            for case in [HexCase::Preserve, HexCase::Lower, HexCase::Upper] {
                let once = normalize_number(input, case);
                assert_eq!(normalize_number(&once, case), once, "{input} @ {case:?}");
            }
        }
    }
}
