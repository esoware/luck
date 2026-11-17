//! The base prefix (`0x`/`0b`) and exponent marker (`e`/`p`) are always
//! lowercased, and a bare leading dot gains a zero (`.5` -> `0.5`).
//!
//! Neither is a style option. Hex digit case follows [`HexCase`]. Digit separators (`_`, Luau)
//! stay, and the value never changes.

use crate::HexCase;

/// Writes `text` normalized to `normalized` and returns true, or returns
/// false when it's already normal.
pub(crate) fn normalize_number(text: &str, hex_case: HexCase, normalized: &mut String) -> bool {
    let (is_hex, prefix, digits) = match text.as_bytes() {
        [b'0', prefix @ (b'x' | b'X'), digits @ ..] => (true, *prefix, digits),
        [b'0', prefix @ (b'b' | b'B'), digits @ ..] => (false, *prefix, digits),
        _ => return normalize_decimal(text, normalized),
    };
    let normal_digit = |b: u8| match b {
        // Hex-float exponent. Its sign and decimal digits fall through
        // unchanged.
        b'p' | b'P' if is_hex => b'p',
        b'a'..=b'f' | b'A'..=b'F' if is_hex => apply_hex_case(b, hex_case),
        _ => b,
    };
    if prefix.is_ascii_lowercase() && digits.iter().all(|&b| normal_digit(b) == b) {
        return false;
    }
    normalized.push_str(if is_hex { "0x" } else { "0b" });
    normalized.extend(digits.iter().map(|&b| char::from(normal_digit(b))));
    true
}

fn normalize_decimal(text: &str, normalized: &mut String) -> bool {
    let should_add_leading_zero = text.starts_with('.');
    if !should_add_leading_zero && !text.bytes().any(|b| b == b'E') {
        return false;
    }
    if should_add_leading_zero {
        normalized.push('0');
    }
    normalized.extend(text.chars().map(|c| if c == 'E' { 'e' } else { c }));
    true
}

const fn apply_hex_case(digit: u8, hex_case: HexCase) -> u8 {
    match hex_case {
        HexCase::Preserve => digit,
        HexCase::Lower => digit.to_ascii_lowercase(),
        HexCase::Upper => digit.to_ascii_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal(text: &str, hex_case: HexCase) -> String {
        let mut normalized = String::new();
        if normalize_number(text, hex_case, &mut normalized) { normalized } else { text.to_owned() }
    }

    #[test]
    fn prefix_always_lowered() {
        assert_eq!(normal("0XB0", HexCase::Preserve), "0xB0");
        assert_eq!(normal("0Xb0", HexCase::Preserve), "0xb0");
        assert_eq!(normal("0B1010", HexCase::Preserve), "0b1010");
    }

    #[test]
    fn digits_follow_config() {
        assert_eq!(normal("0xDEad", HexCase::Preserve), "0xDEad");
        assert_eq!(normal("0xDEad", HexCase::Lower), "0xdead");
        assert_eq!(normal("0xDEad", HexCase::Upper), "0xDEAD");
    }

    #[test]
    fn decimal_exponent_lowered() {
        assert_eq!(normal("1E10", HexCase::Preserve), "1e10");
        assert_eq!(normal("1.5E-3", HexCase::Upper), "1.5e-3");
        assert_eq!(normal("42", HexCase::Upper), "42");
    }

    #[test]
    fn hex_float_exponent_lowered_digits_cased() {
        // `p` marker lowered; `A` digit follows config; `-2` exponent intact.
        assert_eq!(normal("0xAP-2", HexCase::Lower), "0xap-2");
        assert_eq!(normal("0x1.8P3", HexCase::Preserve), "0x1.8p3");
    }

    #[test]
    fn separators_preserved() {
        assert_eq!(normal("0xFF_FF", HexCase::Lower), "0xff_ff");
        assert_eq!(normal("1_000_000", HexCase::Upper), "1_000_000");
    }

    #[test]
    fn leading_dot_gains_zero() {
        assert_eq!(normal(".5", HexCase::Preserve), "0.5");
        assert_eq!(normal(".5E2", HexCase::Preserve), "0.5e2");
        assert_eq!(normal(".5e2", HexCase::Preserve), "0.5e2");
        // Already-zeroed forms and hex floats are untouched.
        assert_eq!(normal("0.5", HexCase::Preserve), "0.5");
        assert_eq!(normal("0x.8p1", HexCase::Preserve), "0x.8p1");
    }

    #[test]
    fn a_normal_number_is_left_as_written() {
        let mut normalized = String::new();
        for text in ["0xff", "0xFF", "0b101", "1e5", "0x1p4", "12"] {
            assert!(!normalize_number(text, HexCase::Preserve, &mut normalized), "{text}");
        }
        assert_eq!(normalized, "");
    }

    #[test]
    fn normalization_is_idempotent() {
        for input in ["0XB0", "0xDEad", "1E10", "0xAP-2", "0B10", ".5", ".5E2"] {
            for case in [HexCase::Preserve, HexCase::Lower, HexCase::Upper] {
                let once = normal(input, case);
                assert_eq!(normal(&once, case), once, "{input} @ {case:?}");
            }
        }
    }
}
