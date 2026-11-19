//! The value of a number literal, as a double, which is how the rules that
//! check one compare it.

use core::ops::Neg;

use luck_ast::Expression;
use luck_token::UnOp;

/// The value of a number literal under any number of unary minuses.
pub(crate) fn signed_number_value(expr: &Expression<'_>) -> Option<f64> {
    match expr {
        Expression::UnaryOp(unary) if unary.op == UnOp::Neg => {
            signed_number_value(&unary.operand).map(Neg::neg)
        }
        Expression::Number(literal) => Some(literal.value.to_f64()),
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => None,
    }
}

/// Whether `expr` is a number literal, with no unary minus, worth `value`.
pub(crate) fn is_number_literal(expr: &Expression<'_>, value: f64) -> bool {
    matches!(expr, Expression::Number(literal) if literal.value.to_f64().total_cmp(&value).is_eq())
}
