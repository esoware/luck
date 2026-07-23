use luck_ast::expr::{Expression, Literal};
use luck_ast::shared::Block;
use luck_ast::transform::AstTransform;
use luck_token::{LuaVersion, NumberSubtypes, Span};

/// Shorten numeric literals using scientific notation, decimal trimming,
/// and hex-to-decimal - version-aware: on Lua 5.3+ the integer/float
/// subtype of every literal is preserved (`1.0` never becomes `1`, an
/// integer never becomes `1e6`), and every candidate must parse back to
/// the identical value before it is accepted.
pub fn shorten(block: Block, version: LuaVersion) -> Block {
    NumberShortener {
        subtypes: crate::expr::number_subtypes(version),
    }
    .transform_block(block)
}

struct NumberShortener {
    subtypes: NumberSubtypes,
}

impl AstTransform for NumberShortener {
    fn transform_expression(&mut self, expr: Expression) -> Expression {
        let expr = self.walk_expression(expr);

        match expr {
            Expression::Number(ref literal) => {
                let shortened = shorten_number(&literal.text, self.subtypes);
                if shortened.len() < literal.text.len() {
                    return Expression::Number(Literal {
                        text: shortened.into(),
                        span: Span::default(),
                    });
                }
                expr
            }
            // Luau
            Expression::Integer(ref literal) => {
                let shortened = shorten_integer(&literal.text);
                if shortened.len() < literal.text.len() {
                    return Expression::Integer(Literal {
                        text: shortened.into(),
                        span: Span::default(),
                    });
                }
                expr
            }
            other => other,
        }
    }
}

/// Shorten a Luau integer literal (`i` suffix). The value is an exact
/// 64-bit pattern, so respelling is lossless. Hex covers every pattern;
/// decimal only up to i64::MAX, since larger values would need a unary
/// minus, which is a different AST shape.
fn shorten_integer(text: &str) -> String {
    let body = &text[..text.len() - 1];
    let digits: String = body
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .trim_start_matches("0b")
        .trim_start_matches("0B")
        .chars()
        .filter(|ch| *ch != '_')
        .collect();
    let radix = match body.as_bytes().get(1) {
        Some(b'x' | b'X') => 16,
        Some(b'b' | b'B') => 2,
        _ => 10,
    };
    let Ok(value) = u64::from_str_radix(&digits, radix) else {
        return text.to_string();
    };

    let mut best = format!("0x{value:x}i");
    if value <= i64::MAX as u64 {
        let decimal = format!("{value}i");
        // Prefer decimal when the spellings tie.
        if decimal.len() <= best.len() {
            best = decimal;
        }
    }
    if best.len() < text.len() {
        best
    } else {
        text.to_string()
    }
}

/// Whether the literal spells a float on 5.3+ (has `.`, `e`, or hex `p`).
fn is_float_form(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' && (bytes[1] | 0x20) == b'x' {
        bytes[2..].iter().any(|&b| b == b'.' || (b | 0x20) == b'p')
    } else {
        bytes.iter().any(|&b| b == b'.' || (b | 0x20) == b'e')
    }
}

/// Round-trip check: the candidate must parse to exactly `value`.
fn roundtrips(candidate: &str, value: f64) -> bool {
    let normalized = if let Some(rest) = candidate.strip_prefix('.') {
        format!("0.{rest}")
    } else if let Some(rest) = candidate.strip_suffix('.') {
        format!("{rest}.0")
    } else {
        candidate.to_string()
    };
    normalized.parse::<f64>() == Ok(value)
}

fn shorten_number(text: &str, subtypes: NumberSubtypes) -> String {
    // A one-character literal can never shrink - and single digits are
    // the most common literals in already-minified rounds.
    if text.len() == 1 {
        return text.to_string();
    }

    let int_subtype = subtypes == NumberSubtypes::IntFloat;

    let bytes = text.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' && (bytes[1] | 0x20) == b'x' {
        let hex = &text[2..];
        if hex.bytes().any(|b| b == b'.' || (b | 0x20) == b'p') {
            // Hex floats are rare and precision-delicate - leave them.
            return text.to_string();
        }
        // Hex integers: the decimal spelling denotes the same integer
        // (both are integer-typed on 5.3+, both f64 elsewhere). Values
        // that wrap past i64::MAX have no positive decimal spelling.
        if let Ok(value) = u64::from_str_radix(hex, 16)
            && value <= i64::MAX as u64
        {
            let decimal = itoa::Buffer::new().format(value).to_string();
            if decimal.len() < text.len() && (int_subtype || roundtrips(&decimal, value as f64)) {
                return decimal;
            }
        }
        return text.to_string();
    }

    if bytes.len() >= 2 && bytes[0] == b'0' && (bytes[1] | 0x20) == b'b' {
        return text.to_string();
    }

    let Ok(value) = text.parse::<f64>() else {
        return text.to_string();
    };
    if !value.is_finite() {
        return text.to_string();
    }

    let float_form = is_float_form(text);
    let mut candidates: Vec<String> = Vec::new();

    // Integer spelling - only when the literal already IS an integer on
    // 5.3+ (or subtypes don't exist). `1.0` -> `1` flips math.type.
    if value == value.floor() && value.abs() < 1e15 && (!int_subtype || !float_form) {
        candidates.push(itoa::Buffer::new().format(value as i64).to_string());
    }

    // Float-preserving spellings for float-formed literals: `2.0` -> `2.`
    // and `0.5` -> `.5` keep the subtype; scientific keeps it too.
    if !int_subtype || float_form {
        let decimal = format_shortest_decimal(value, int_subtype && float_form);
        candidates.push(decimal);

        if value != 0.0 {
            let exp = value.abs().log10().floor() as i32;
            if exp >= 3 || exp <= -2 {
                let mantissa = value / 10f64.powi(exp);
                candidates.push(format!(
                    "{}e{exp}",
                    format_shortest_decimal(mantissa, false)
                ));
            }
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
        .min_by_key(|candidate| candidate.len())
        .filter(|candidate| candidate.len() < text.len())
        .unwrap_or_else(|| text.to_string())
}

fn format_shortest_decimal(value: f64, must_stay_float_formed: bool) -> String {
    if value == 0.0 {
        return if must_stay_float_formed { "0." } else { "0" }.to_string();
    }

    // Shortest-roundtrip digits without the fmt machinery. ryu uses
    // exponent form for extreme magnitudes where Display does not; the
    // 'e'-aware paths below handle both, and every candidate still passes
    // the roundtrips() gate.
    let mut ryu_buffer = ryu::Buffer::new();
    let formatted = ryu_buffer.format(value);

    // Exponent forms are already float-formed and must skip the trailing-
    // zero trim, which would otherwise eat exponent digits ("1.5e300").
    if formatted.contains('e') || formatted.contains('E') {
        return formatted.to_string();
    }

    if formatted.contains('.') {
        let trimmed = formatted.trim_end_matches('0');
        let trimmed = if must_stay_float_formed {
            // Keep the `.` so the literal still spells a float (`2.`).
            trimmed
        } else {
            trimmed.trim_end_matches('.')
        };
        if let Some(rest) = trimmed.strip_prefix("0.") {
            return format!(".{rest}");
        }
        return trimmed.to_string();
    }

    if must_stay_float_formed {
        return format!("{formatted}.");
    }
    formatted.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subtype_trailing_zero_drops() {
        assert_eq!(shorten_number("1.0", NumberSubtypes::Unified), "1");
        assert_eq!(shorten_number("2.0", NumberSubtypes::Unified), "2");
        assert_eq!(shorten_number("10.0", NumberSubtypes::Unified), "10");
    }

    #[test]
    fn subtype_float_keeps_float_form() {
        // On 5.3+, `1.0` -> `1` flips math.type - `1.` keeps it float.
        assert_eq!(shorten_number("1.0", NumberSubtypes::IntFloat), "1.");
        assert_eq!(shorten_number("10.0", NumberSubtypes::IntFloat), "10.");
    }

    #[test]
    fn subtype_integer_never_goes_scientific() {
        // `1e6` is a float on 5.3+; the integer must stay spelled as one.
        assert_eq!(
            shorten_number("1000000", NumberSubtypes::IntFloat),
            "1000000"
        );
        assert_eq!(shorten_number("100000", NumberSubtypes::IntFloat), "100000");
    }

    #[test]
    fn no_subtype_scientific_ok() {
        assert_eq!(shorten_number("100000", NumberSubtypes::Unified), "1e5");
        assert_eq!(shorten_number("1000000", NumberSubtypes::Unified), "1e6");
    }

    #[test]
    fn leading_zero_drops_either_way() {
        assert_eq!(shorten_number("0.5", NumberSubtypes::Unified), ".5");
        assert_eq!(shorten_number("0.5", NumberSubtypes::IntFloat), ".5");
        assert_eq!(shorten_number("0.25", NumberSubtypes::IntFloat), ".25");
    }

    #[test]
    fn no_change_needed() {
        assert_eq!(shorten_number("1", NumberSubtypes::IntFloat), "1");
        assert_eq!(shorten_number("42", NumberSubtypes::Unified), "42");
        assert_eq!(shorten_number("0", NumberSubtypes::IntFloat), "0");
    }

    #[test]
    fn hex_to_decimal_both_models() {
        assert_eq!(shorten_number("0xFF", NumberSubtypes::IntFloat), "255");
        assert_eq!(shorten_number("0xFF", NumberSubtypes::Unified), "255");
        assert_eq!(shorten_number("0x1A", NumberSubtypes::IntFloat), "26");
        assert_eq!(
            shorten_number("0xFFFFFFFF", NumberSubtypes::IntFloat),
            "0xFFFFFFFF"
        );
    }

    #[test]
    fn wrapping_hex_kept() {
        // 0xFFFFFFFFFFFFFFFF is -1 on 5.3+; no positive decimal spelling.
        assert_eq!(
            shorten_number("0xFFFFFFFFFFFFFFFF", NumberSubtypes::IntFloat),
            "0xFFFFFFFFFFFFFFFF"
        );
    }

    #[test]
    fn integer_literals_shorten_losslessly() {
        // Luau
        assert_eq!(shorten_integer("1_000_000i"), "1000000i");
        assert_eq!(shorten_integer("0xffi"), "255i");
        assert_eq!(shorten_integer("0b1111i"), "15i");
        assert_eq!(shorten_integer("0XFF_FFi"), "65535i");
        // Bit patterns above i64::MAX have no decimal spelling; hex is
        // still shorter than binary.
        assert_eq!(
            shorten_integer("0b1111111111111111111111111111111111111111111111111111111111111111i"),
            "0xffffffffffffffffi"
        );
        assert_eq!(
            shorten_integer("0xFFFF_FFFF_FFFF_FFFFi"),
            "0xffffffffffffffffi"
        );
        // Already-shortest spellings stay put (idempotent).
        assert_eq!(shorten_integer("255i"), "255i");
        assert_eq!(shorten_integer("0i"), "0i");
        assert_eq!(
            shorten_integer("0xffffffffffffffffi"),
            "0xffffffffffffffffi"
        );
        assert_eq!(shorten_integer("1000000i"), "1000000i");
    }
}
