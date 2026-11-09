//! Values of number literals, for the rules that check literal bounds and
//! indices.
//!
//! A spelling `parse_lua_number` refuses, such as a hex float, yields `None`.

use core::ops::Neg;

use luck_ast::Expression;
use luck_token::{LuaNumber, NumberSubtypes, UnOp, parse_lua_number};

/// The value of a number literal under any number of unary minuses.
pub(crate) fn signed_number_value(expr: &Expression) -> Option<f64> {
    if let Expression::UnaryOp(unary) = expr
        && unary.op == UnOp::Neg
    {
        return signed_number_value(&unary.operand).map(Neg::neg);
    }
    unsigned_number_value(expr)
}

/// Whether `expr` is a number literal, with no unary minus, worth `value`.
pub(crate) fn is_number_literal(expr: &Expression, value: f64) -> bool {
    unsigned_number_value(expr) == Some(value)
}

fn unsigned_number_value(expr: &Expression) -> Option<f64> {
    let Expression::Number(literal) = expr else {
        return None;
    };
    match parse_lua_number(&literal.text, NumberSubtypes::IntFloat)? {
        #[expect(
            clippy::as_conversions,
            clippy::cast_precision_loss,
            reason = "the rules compare values as doubles, so this rounding is the conversion wanted"
        )]
        LuaNumber::Int(int_value) => Some(int_value as f64),
        LuaNumber::Float(float_value) => Some(float_value),
    }
}
