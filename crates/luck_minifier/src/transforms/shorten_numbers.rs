use luck_ast::expr::Expression;
use luck_ast::shared::Block;
use luck_ast::transform::AstTransform;
use luck_token::token::{Token, TokenKind};
use luck_token::{LuaVersion, Span};

/// Shorten numeric literals using scientific notation, decimal trimming,
/// and hex-to-decimal - version-aware: on Lua 5.3+ the integer/float
/// subtype of every literal is preserved (`1.0` never becomes `1`, an
/// integer never becomes `1e6`), and every candidate must parse back to
/// the identical value before it is accepted.
pub fn shorten(block: Block, version: LuaVersion) -> Block {
    NumberShortener {
        int_subtype: version.has_integer_subtype(),
    }
    .transform_block(block)
}

struct NumberShortener {
    int_subtype: bool,
}

impl AstTransform for NumberShortener {
    fn transform_expression(&mut self, expr: Expression) -> Expression {
        let expr = self.walk_expression(expr);

        match expr {
            Expression::Number(ref token) => {
                if let TokenKind::Number(ref text) = token.kind {
                    let shortened = shorten_number(text, self.int_subtype);
                    if shortened.len() < text.len() {
                        return Expression::Number(Token::new(
                            TokenKind::Number(shortened.into()),
                            Span::default(),
                        ));
                    }
                }
                expr
            }
            other => other,
        }
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

fn shorten_number(text: &str, int_subtype: bool) -> String {
    // A one-character literal can never shrink - and single digits are
    // the most common literals in already-minified rounds.
    if text.len() == 1 {
        return text.to_string();
    }

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
        if let Ok(value) = u64::from_str_radix(hex, 16) {
            if value <= i64::MAX as u64 {
                let decimal = itoa::Buffer::new().format(value).to_string();
                if decimal.len() < text.len() && (int_subtype || roundtrips(&decimal, value as f64))
                {
                    return decimal;
                }
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

    let formatted = format!("{value}");

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

    if must_stay_float_formed && !formatted.contains('e') && !formatted.contains('E') {
        return format!("{formatted}.");
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subtype_trailing_zero_drops() {
        assert_eq!(shorten_number("1.0", false), "1");
        assert_eq!(shorten_number("2.0", false), "2");
        assert_eq!(shorten_number("10.0", false), "10");
    }

    #[test]
    fn subtype_float_keeps_float_form() {
        // On 5.3+, `1.0` -> `1` flips math.type - `1.` keeps it float.
        assert_eq!(shorten_number("1.0", true), "1.");
        assert_eq!(shorten_number("10.0", true), "10.");
    }

    #[test]
    fn subtype_integer_never_goes_scientific() {
        // `1e6` is a float on 5.3+; the integer must stay spelled as one.
        assert_eq!(shorten_number("1000000", true), "1000000");
        assert_eq!(shorten_number("100000", true), "100000");
    }

    #[test]
    fn no_subtype_scientific_ok() {
        assert_eq!(shorten_number("100000", false), "1e5");
        assert_eq!(shorten_number("1000000", false), "1e6");
    }

    #[test]
    fn leading_zero_drops_either_way() {
        assert_eq!(shorten_number("0.5", false), ".5");
        assert_eq!(shorten_number("0.5", true), ".5");
        assert_eq!(shorten_number("0.25", true), ".25");
    }

    #[test]
    fn no_change_needed() {
        assert_eq!(shorten_number("1", true), "1");
        assert_eq!(shorten_number("42", false), "42");
        assert_eq!(shorten_number("0", true), "0");
    }

    #[test]
    fn hex_to_decimal_both_models() {
        assert_eq!(shorten_number("0xFF", true), "255");
        assert_eq!(shorten_number("0xFF", false), "255");
        assert_eq!(shorten_number("0x1A", true), "26");
        assert_eq!(shorten_number("0xFFFFFFFF", true), "0xFFFFFFFF");
    }

    #[test]
    fn wrapping_hex_kept() {
        // 0xFFFFFFFFFFFFFFFF is -1 on 5.3+; no positive decimal spelling.
        assert_eq!(
            shorten_number("0xFFFFFFFFFFFFFFFF", true),
            "0xFFFFFFFFFFFFFFFF"
        );
    }
}
