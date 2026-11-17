//! Respelling a number literal shorter.
//!
//! It keeps its value and, on Lua 5.3+, its integer or float subtype: `1.0`
//! never becomes `1`, an integer never becomes `1e6`, and a candidate is
//! only used if it parses back to the same value.

use luck_allocator::Allocator;
use luck_ast::Literal;
use luck_token::NumberSubtypes;

use super::literal_cache::LiteralCache;
use crate::expr::int_to_float;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the cache is one type with its respellings split by flag"
)]
impl LiteralCache {
    pub(super) fn shorten_number<'ast>(
        &mut self,
        allocator: &'ast Allocator,
        subtypes: NumberSubtypes,
        literal: &mut Literal<'ast>,
    ) -> bool {
        // A one-character literal can never shrink.
        if literal.text.len() == 1 || is_shortest_integer(literal.text, subtypes) {
            return false;
        }
        self.shorten(allocator, literal, |text, respelled| {
            shorten_number(text, subtypes).is_some_and(|shortened| {
                respelled.push_str(&shortened);
                true
            })
        })
    }

    /// Luau: an exact 64-bit integer literal.
    pub(super) fn shorten_integer<'ast>(
        &mut self,
        allocator: &'ast Allocator,
        literal: &mut Literal<'ast>,
    ) -> bool {
        self.shorten(allocator, literal, |text, respelled| {
            shorten_integer(text).is_some_and(|shortened| {
                respelled.push_str(&shortened);
                true
            })
        })
    }
}

/// The value is an exact 64-bit pattern, so respelling is lossless. Hex covers
/// every pattern; decimal only up to `i64::MAX`, since larger values would
/// need a unary minus, which is a different AST shape.
fn shorten_integer(text: &str) -> Option<String> {
    let body = text.strip_suffix('i').unwrap_or(text);
    // Twelve decimal digits are never longer than the hex spelling.
    if body.len() <= 12 && !body.starts_with('0') && body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Luau drops every `_`, even after the suffix, as in `1i_`.
    let spelled: String = text.chars().filter(|c| *c != '_').collect();
    let spelled_body = spelled.strip_suffix('i').unwrap_or(&spelled);
    let binary_digits = spelled_body.strip_prefix("0b").or_else(|| spelled_body.strip_prefix("0B"));
    let (digits, radix) = match (hex_digits(spelled_body), binary_digits) {
        (Some(hex), _) => (hex, 16),
        (None, Some(binary)) => (binary, 2),
        (None, None) => (spelled_body, 10),
    };
    let value = u64::from_str_radix(digits, radix).ok()?;

    let mut best = format!("0x{value:x}i");
    if i64::try_from(value).is_ok() {
        let decimal = format!("{value}i");
        // Prefer decimal when the spellings tie.
        if decimal.len() <= best.len() {
            best = decimal;
        }
    }
    (best.len() < text.len()).then_some(best)
}

fn hex_digits(text: &str) -> Option<&str> {
    text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")).filter(|digits| !digits.is_empty())
}

/// Whether the literal spells a float on 5.3+ (has `.`, `e`, or hex `p`).
fn is_float_form(text: &str) -> bool {
    hex_digits(text)
        .map_or_else(|| text.contains(['.', 'e', 'E']), |digits| digits.contains(['.', 'p', 'P']))
}

/// Rust's float parser accepts Lua's `.5` and `5.` spellings.
fn roundtrips(candidate: &str, value: f64) -> bool {
    candidate.parse::<f64>() == Ok(value)
}

/// Whether `text` is a decimal integer no other spelling beats: one without
/// a leading zero, exact as a float, and on a target with no integer subtype,
/// too few trailing zeros for an exponent to pay off.
fn is_shortest_integer(text: &str, subtypes: NumberSubtypes) -> bool {
    let is_plain =
        text.len() <= 15 && !text.starts_with('0') && text.bytes().all(|b| b.is_ascii_digit());
    is_plain
        && (subtypes == NumberSubtypes::IntFloat
            || text.bytes().rev().take_while(|&b| b == b'0').count() < 3)
}

fn shorten_number(text: &str, subtypes: NumberSubtypes) -> Option<String> {
    // A one-character literal can never shrink.
    if text.len() == 1 || is_shortest_integer(text, subtypes) {
        return None;
    }

    let has_int_subtype = subtypes == NumberSubtypes::IntFloat;

    if let Some(digits) = hex_digits(text) {
        if digits.contains(['.', 'p', 'P']) {
            // Hex floats are rare and precision-delicate, so leave them.
            return None;
        }
        // Hex integers: the decimal spelling denotes the same integer
        // (both are integer-typed on 5.3+, both f64 elsewhere). Values
        // that wrap past i64::MAX have no positive decimal spelling.
        if let Ok(value) = u64::from_str_radix(digits, 16)
            && let Ok(integer) = i64::try_from(value)
        {
            let decimal = itoa::Buffer::new().format(integer).to_owned();
            if decimal.len() < text.len()
                && (has_int_subtype || roundtrips(&decimal, int_to_float(integer)))
            {
                return Some(decimal);
            }
        }
        return None;
    }

    if text.starts_with("0b") || text.starts_with("0B") {
        return None;
    }

    let value = text.parse::<f64>().ok()?;
    if !value.is_finite() {
        return None;
    }

    let is_float_formed = is_float_form(text);
    let mut candidates: Vec<String> = Vec::new();

    // Integer spelling, only when the literal is already an integer on 5.3+
    // or the target has no subtypes. Shortening `1.0` to `1` flips math.type.
    if value.fract() == 0.0f64 && value.abs() < 1e15f64 && (!has_int_subtype || !is_float_formed) {
        candidates.push(format_shortest_decimal(value, false));
    }

    // Float-preserving spellings for float-formed literals: `2.0` -> `2.`
    // and `0.5` -> `.5` keep the subtype; scientific keeps it too.
    if !has_int_subtype || is_float_formed {
        let decimal = format_shortest_decimal(value, has_int_subtype && is_float_formed);
        candidates.push(decimal);

        // Rust prints the shortest digits that round-trip, the same on every
        // host. Within this range a decimal spelling is never longer.
        if !(0.1f64..100.0f64).contains(&value.abs()) {
            candidates.push(format!("{value:e}"));
        }
        // `0.5` -> `.5`, but never `0.` -> `.` (not a number literal).
        if let Some(stripped) = text.strip_prefix("0.")
            && !stripped.is_empty()
        {
            candidates.push(format!(".{stripped}"));
        }
    }

    candidates
        .into_iter()
        .filter(|candidate| roundtrips(candidate, value))
        .min_by_key(String::len)
        .filter(|candidate| candidate.len() < text.len())
}

fn format_shortest_decimal(value: f64, should_stay_float_formed: bool) -> String {
    if value == 0.0f64 {
        return if should_stay_float_formed { "0." } else { "0" }.to_owned();
    }

    // ryu gives the shortest round-trip digits without the fmt machinery, but
    // switches to exponent form at extreme magnitudes. Every candidate still
    // goes through roundtrips().
    let mut ryu_buffer = ryu::Buffer::new();
    let formatted = ryu_buffer.format(value);

    // Exponent forms are already float-formed and must skip the trailing-
    // zero trim, which would otherwise eat exponent digits ("1.5e300").
    if formatted.contains(['e', 'E']) {
        return formatted.to_owned();
    }

    if formatted.contains('.') {
        let without_zeros = formatted.trim_end_matches('0');
        let trimmed = if should_stay_float_formed {
            // Keep the `.` so the literal still spells a float (`2.`).
            without_zeros
        } else {
            without_zeros.trim_end_matches('.')
        };
        if let Some(rest) = trimmed.strip_prefix("0.") {
            return format!(".{rest}");
        }
        return trimmed.to_owned();
    }

    if should_stay_float_formed {
        return format!("{formatted}.");
    }
    formatted.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subtype_trailing_zero_drops() {
        assert_eq!(shorten_number("1.0", NumberSubtypes::Unified).as_deref(), Some("1"));
        assert_eq!(shorten_number("2.0", NumberSubtypes::Unified).as_deref(), Some("2"));
        assert_eq!(shorten_number("10.0", NumberSubtypes::Unified).as_deref(), Some("10"));
    }

    #[test]
    fn subtype_float_keeps_float_form() {
        // On 5.3+, `1.0` shortened to `1` flips math.type, while `1.` keeps
        // it float.
        assert_eq!(shorten_number("1.0", NumberSubtypes::IntFloat).as_deref(), Some("1."));
        assert_eq!(shorten_number("10.0", NumberSubtypes::IntFloat).as_deref(), Some("10."));
    }

    #[test]
    fn subtype_integer_never_goes_scientific() {
        // `1e6` is a float on 5.3+; the integer must stay spelled as one.
        assert_eq!(shorten_number("1000000", NumberSubtypes::IntFloat), None);
        assert_eq!(shorten_number("100000", NumberSubtypes::IntFloat), None);
    }

    #[test]
    fn scientific_spelling_keeps_the_shortest_digits() {
        for exponent in -307i32..=308i32 {
            let literal = format!("1.0e{exponent}");
            let shortest = shorten_number(&literal, NumberSubtypes::IntFloat).unwrap_or_default();
            assert!(shortest.len() <= format!("1e{exponent}").len(), "{literal}: {shortest}");
            assert_eq!(shortest.parse::<f64>(), literal.parse::<f64>(), "{literal}");
        }
        for zeros in 3usize..=15 {
            let literal = format!("1{}", "0".repeat(zeros));
            let shortened = shorten_number(&literal, NumberSubtypes::Unified);
            assert_eq!(shortened, Some(format!("1e{zeros}")), "{literal}");
        }
        assert_eq!(shorten_number("0.0001", NumberSubtypes::IntFloat).as_deref(), Some("1e-4"));
        assert_eq!(shorten_number("1500000.0", NumberSubtypes::IntFloat).as_deref(), Some("1.5e6"));
        assert_eq!(
            shorten_number("0.000010012", NumberSubtypes::IntFloat).as_deref(),
            Some("1.0012e-5")
        );
        assert_eq!(shorten_number("999.9999999999999", NumberSubtypes::IntFloat), None);
        // A float-formed `100.` is one byte longer than `1e2`.
        assert_eq!(shorten_number("100.0", NumberSubtypes::IntFloat).as_deref(), Some("1e2"));
        assert_eq!(shorten_number("100.0", NumberSubtypes::Unified).as_deref(), Some("100"));
    }

    #[test]
    fn no_subtype_goes_scientific() {
        assert_eq!(shorten_number("100000", NumberSubtypes::Unified).as_deref(), Some("1e5"));
        assert_eq!(shorten_number("1000000", NumberSubtypes::Unified).as_deref(), Some("1e6"));
    }

    #[test]
    fn leading_zero_drops_either_way() {
        assert_eq!(shorten_number("0.5", NumberSubtypes::Unified).as_deref(), Some(".5"));
        assert_eq!(shorten_number("0.5", NumberSubtypes::IntFloat).as_deref(), Some(".5"));
        assert_eq!(shorten_number("0.25", NumberSubtypes::IntFloat).as_deref(), Some(".25"));
        assert_eq!(shorten_number("007", NumberSubtypes::IntFloat).as_deref(), Some("7"));
    }

    #[test]
    fn shortest_spelling_has_no_rewrite() {
        assert_eq!(shorten_number("1", NumberSubtypes::IntFloat), None);
        assert_eq!(shorten_number("42", NumberSubtypes::Unified), None);
        assert_eq!(shorten_number("0", NumberSubtypes::IntFloat), None);
        assert_eq!(shorten_number("1200", NumberSubtypes::Unified), None);
    }

    #[test]
    fn a_long_integer_shortens_to_its_exponent_where_floats_are_the_only_numbers() {
        let literal = format!("12345678901234567{}", "0".repeat(13));
        assert_eq!(
            shorten_number(&literal, NumberSubtypes::Unified).as_deref(),
            Some("1.2345678901234568e29")
        );
    }

    #[test]
    fn hex_to_decimal_both_models() {
        assert_eq!(shorten_number("0xFF", NumberSubtypes::IntFloat).as_deref(), Some("255"));
        assert_eq!(shorten_number("0xFF", NumberSubtypes::Unified).as_deref(), Some("255"));
        assert_eq!(shorten_number("0x1A", NumberSubtypes::IntFloat).as_deref(), Some("26"));
        assert_eq!(shorten_number("0xFFFFFFFF", NumberSubtypes::IntFloat), None);
    }

    #[test]
    fn wrapping_hex_kept() {
        // 0xFFFFFFFFFFFFFFFF is -1 on 5.3+; no positive decimal spelling.
        assert_eq!(shorten_number("0xFFFFFFFFFFFFFFFF", NumberSubtypes::IntFloat), None);
    }

    #[test]
    fn integer_literals_shorten_losslessly() {
        // Luau
        assert_eq!(shorten_integer("1_000_000i").as_deref(), Some("1000000i"));
        assert_eq!(shorten_integer("0xffi").as_deref(), Some("255i"));
        assert_eq!(shorten_integer("0b1111i").as_deref(), Some("15i"));
        assert_eq!(shorten_integer("0XFF_FFi").as_deref(), Some("65535i"));
        assert_eq!(shorten_integer("1i_").as_deref(), Some("1i"));
        assert_eq!(shorten_integer("0_x1_0i__").as_deref(), Some("16i"));
        // Bit patterns above i64::MAX have no decimal spelling; hex is
        // still shorter than binary.
        assert_eq!(
            shorten_integer("0b1111111111111111111111111111111111111111111111111111111111111111i")
                .as_deref(),
            Some("0xffffffffffffffffi")
        );
        assert_eq!(
            shorten_integer("0xFFFF_FFFF_FFFF_FFFFi").as_deref(),
            Some("0xffffffffffffffffi")
        );
        assert_eq!(shorten_integer("1000000000000000000i").as_deref(), Some("0xde0b6b3a7640000i"));
        // Already-shortest spellings stay put (idempotent).
        assert_eq!(shorten_integer("255i"), None);
        assert_eq!(shorten_integer("0i"), None);
        assert_eq!(shorten_integer("0xffffffffffffffffi"), None);
        assert_eq!(shorten_integer("1000000i"), None);
    }

    #[test]
    fn hex_integer_starting_with_0b_keeps_its_value() {
        assert_eq!(
            shorten_integer("0x0b12i").as_deref(),
            Some("2834i"),
            "the digits after `0x` must not lose a leading `0b`"
        );
    }
}
