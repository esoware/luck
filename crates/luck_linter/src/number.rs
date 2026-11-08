//! Literal bounds for the rules that check numeric `for` ranges.

use core::ops::Neg;

use luck_ast::Expression;
use luck_token::UnOp;

/// The value of a number literal under any number of unary minuses. Rust's
/// float parser reads the text, so hex literals yield `None`.
pub(crate) fn decimal_literal_value(expr: &Expression) -> Option<f64> {
    if let Expression::UnaryOp(unary) = expr
        && unary.op == UnOp::Neg
    {
        return decimal_literal_value(&unary.operand).map(Neg::neg);
    }
    let Expression::Number(literal) = expr else {
        return None;
    };
    literal.text.parse().ok()
}
