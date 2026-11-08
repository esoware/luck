//! Values of number literals, for the rules that check literal bounds and
//! indices.
//!
//! Rust's float parser reads the literal text, so hex literals yield `None`.

use core::ops::Neg;

use luck_ast::Expression;
use luck_token::UnOp;

/// The value of a number literal under any number of unary minuses.
pub(crate) fn decimal_literal_value(expr: &Expression) -> Option<f64> {
    if let Expression::UnaryOp(unary) = expr
        && unary.op == UnOp::Neg
    {
        return decimal_literal_value(&unary.operand).map(Neg::neg);
    }
    unsigned_decimal_value(expr)
}

/// Whether `expr` is a number literal, with no unary minus, worth `value`.
pub(crate) fn is_decimal_literal(expr: &Expression, value: f64) -> bool {
    unsigned_decimal_value(expr) == Some(value)
}

fn unsigned_decimal_value(expr: &Expression) -> Option<f64> {
    let Expression::Number(literal) = expr else {
        return None;
    };
    literal.text.parse().ok()
}
