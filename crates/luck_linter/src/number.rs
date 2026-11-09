//! Values of number literals, for the rules that check literal bounds and
//! indices.
//!
//! Every rule reads a literal through here, so Luau's `_` separators and
//! binary literals and each dialect's hex overflow live in one place. A
//! spelling this doesn't model, such as a nonzero hex float, yields `None`.

use alloc::borrow::Cow;
use core::ops::Neg;

use luck_ast::Expression;
use luck_token::{LuaVersion, UnOp};

/// The digits of an integer literal and their radix.
///
/// Decimal text overflows past 64 bits into a float on 5.3+, hex wraps, and
/// either loses precision as a double elsewhere, so the rules that care read
/// these rather than a value.
pub(crate) struct IntegerDigits<'text> {
    pub digits: Cow<'text, str>,
    pub radix: u32,
}

/// The digits of a number literal's text, separators gone, or `None` for a
/// float spelling.
pub(crate) fn integer_digits(text: &str) -> Option<IntegerDigits<'_>> {
    let (body, radix) = match text.as_bytes() {
        [b'0', b'x' | b'X', ..] => (text.get(2..)?, 16),
        [b'0', b'b' | b'B', ..] => (text.get(2..)?, 2),
        _ => (text, 10),
    };
    let digits = without_separators(body);
    let is_integer = !digits.is_empty()
        && digits
            .bytes()
            .all(|byte| if radix == 16 { byte.is_ascii_hexdigit() } else { byte.is_ascii_digit() });
    is_integer.then_some(IntegerDigits { digits, radix })
}

/// The value of a number literal under any number of unary minuses.
pub(crate) fn signed_number_value(expr: &Expression, version: LuaVersion) -> Option<f64> {
    if let Expression::UnaryOp(unary) = expr
        && unary.op == UnOp::Neg
    {
        return signed_number_value(&unary.operand, version).map(Neg::neg);
    }
    unsigned_number_value(expr, version)
}

/// Whether `expr` is a number literal, with no unary minus, worth `value`.
pub(crate) fn is_number_literal(expr: &Expression, value: f64, version: LuaVersion) -> bool {
    unsigned_number_value(expr, version) == Some(value)
}

fn unsigned_number_value(expr: &Expression, version: LuaVersion) -> Option<f64> {
    let Expression::Number(literal) = expr else {
        return None;
    };
    let text = literal.text.as_str();
    let Some(IntegerDigits { digits, radix }) = integer_digits(text) else {
        return float_value(text);
    };
    if radix == 10 {
        return digits.parse().ok();
    }
    let magnitude = u128::from_str_radix(&digits, radix).ok()?;
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "the rules compare values as doubles, so this rounding is the conversion wanted"
    )]
    let value = if version.has_integer_subtype() {
        // Lua 5.3+ wraps a hex literal into the integer range.
        (magnitude as u64).cast_signed() as f64
    } else {
        magnitude as f64
    };
    Some(value)
}

fn float_value(text: &str) -> Option<f64> {
    let Some(hex_float) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) else {
        return without_separators(text).parse().ok();
    };
    // Only a zero hex float has a value that doesn't need correct rounding.
    let mantissa = hex_float.split(['p', 'P']).next()?;
    mantissa.bytes().all(|byte| byte == b'0' || byte == b'.').then_some(0.0)
}

fn without_separators(text: &str) -> Cow<'_, str> {
    if text.contains('_') { Cow::Owned(text.replace('_', "")) } else { Cow::Borrowed(text) }
}
