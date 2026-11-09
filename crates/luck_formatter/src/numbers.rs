//! The base prefix (`0x`/`0b`) and exponent marker (`e`/`p`) are always
//! lowercased, and a bare leading dot gains a zero (`.5` -> `0.5`).
//!
//! Neither is a style option. Hex digit case follows [`HexCase`]. Digit separators (`_`, Luau)
//! stay, and the value never changes.

use alloc::borrow::Cow;

use crate::HexCase;

pub(crate) fn normalize_number(text: &str, hex_case: HexCase) -> Cow<'_, str> {
    let (is_hex, digits) = match text.as_bytes() {
        [b'0', b'x' | b'X', digits @ ..] => (true, digits),
        [b'0', b'b' | b'B', digits @ ..] => (false, digits),
        _ => return normalize_decimal(text),
    };

    let mut normalized = String::with_capacity(text.len());
    normalized.push_str(if is_hex { "0x" } else { "0b" });

    for &b in digits {
        let c = char::from(b);
        match b {
            // Hex-float exponent. Its sign and decimal digits fall through
            // unchanged.
            b'p' | b'P' if is_hex => normalized.push('p'),
            b'a'..=b'f' | b'A'..=b'F' if is_hex => {
                normalized.push(apply_hex_case(c, hex_case));
            }
            _ => normalized.push(c),
        }
    }
    Cow::Owned(normalized)
}

fn normalize_decimal(text: &str) -> Cow<'_, str> {
    let should_add_leading_zero = text.starts_with('.');
    if !should_add_leading_zero && !text.bytes().any(|b| b == b'E') {
        return Cow::Borrowed(text);
    }
    let mut normalized = String::with_capacity(text.len().saturating_add(1));
    if should_add_leading_zero {
        normalized.push('0');
    }
    normalized.extend(text.chars().map(|c| if c == 'E' { 'e' } else { c }));
    Cow::Owned(normalized)
}

const fn apply_hex_case(character: char, hex_case: HexCase) -> char {
    match hex_case {
        HexCase::Preserve => character,
        HexCase::Lower => character.to_ascii_lowercase(),
        HexCase::Upper => character.to_ascii_uppercase(),
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
    fn normalization_is_idempotent() {
        for input in ["0XB0", "0xDEad", "1E10", "0xAP-2", "0B10", ".5", ".5E2"] {
            for case in [HexCase::Preserve, HexCase::Lower, HexCase::Upper] {
                let once = normalize_number(input, case);
                assert_eq!(normalize_number(&once, case), once, "{input} @ {case:?}");
            }
        }
    }
}
