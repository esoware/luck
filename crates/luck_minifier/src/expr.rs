//! Expression predicates and builders the passes share.

use luck_ast::{Expression, Field, ParenthesizedExpression, Var};
use luck_token::{BinOp, OperandSide, Span, UnOp};

pub(crate) fn extract_boolean(expr: &Expression) -> Option<bool> {
    match expr {
        Expression::True(_) => Some(true),
        Expression::False(_) => Some(false),
        Expression::Parenthesized(paren) => extract_boolean(&paren.expr),
        Expression::Nil(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => None,
    }
}

pub(crate) fn is_nil(expr: &Expression) -> bool {
    match expr {
        Expression::Nil(_) => true,
        Expression::Parenthesized(paren) => is_nil(&paren.expr),
        Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => false,
    }
}

/// Whether the expression is built only from literal values, with no variable
/// reads. Nested arithmetic on literals is still literal, because metamethods
/// can't fire on primitives.
fn is_literal_expression(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::Parenthesized(paren) => is_literal_expression(&paren.expr),
        Expression::UnaryOp(unop) => is_literal_expression(&unop.operand),
        Expression::BinaryOp(binop) => {
            is_literal_expression(&binop.left) && is_literal_expression(&binop.right)
        }
        Expression::Integer(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => false,
    }
}

/// Whether evaluating `expr` has no side effects.
///
/// A plain name read counts as pure. Any operator but `and`/`or` counts only
/// when it applies to literals alone, since anything else may run a metamethod.
pub(crate) fn is_pure_expression(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_) // Luau integers are exact literal values.
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::FunctionDef(_)
        // `...` can't be reassigned, so its value is fixed for the whole
        // function body.
        | Expression::VarArg(_) => true,
        Expression::Parenthesized(paren) => is_pure_expression(&paren.expr),
        Expression::UnaryOp(_) => is_literal_expression(expr),
        Expression::BinaryOp(binop) => {
            if matches!(binop.op, BinOp::And | BinOp::Or) {
                // and/or never invoke metamethods, so they are pure whenever
                // both operands are.
                is_pure_expression(&binop.left) && is_pure_expression(&binop.right)
            } else {
                is_literal_expression(expr)
            }
        }
        Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
            Field::Bracketed { key, value, .. } => {
                is_pure_expression(key) && is_pure_expression(value)
            }
            Field::Named { value, .. } | Field::Positional { value, .. } => {
                is_pure_expression(value)
            }
        }),
        Expression::Var(var) => match var {
            Var::Name(_) => true,
            // Field access can raise on a nil prefix or fire __index, just
            // like `Var::Index`.
            Var::FieldAccess(_) | Var::Index(_) => false,
        },
        // Luau type casts are transparent wrappers, so purity is the inner
        // expression's.
        Expression::TypeCast(cast) => is_pure_expression(&cast.expr),
        // Luau: explicit type arguments aren't evaluated at runtime.
        Expression::TypeInstantiation(instantiation) => is_pure_expression(&instantiation.expr),
        Expression::FunctionCall(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => false,
    }
}

/// Whether the expression is guaranteed truthy at runtime. Only `nil` and
/// `false` are falsy in Lua, so numbers, strings, tables, and functions pass.
pub(crate) fn is_always_truthy(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::FunctionDef(_)
        | Expression::TableConstructor(_)
        | Expression::True(_) => true,
        Expression::Parenthesized(paren) => is_always_truthy(&paren.expr),
        Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Neg) => {
            is_literal_expression(&unop.operand) && is_always_truthy(&unop.operand)
        }
        Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Len) => {
            is_literal_expression(&unop.operand)
        }
        Expression::BinaryOp(binop) => {
            let is_arithmetic = matches!(
                binop.op,
                BinOp::Add
                    | BinOp::Sub
                    | BinOp::Mul
                    | BinOp::Div
                    | BinOp::FloorDiv
                    | BinOp::Mod
                    | BinOp::Pow
                    | BinOp::Concat
                    | BinOp::BitAnd
                    | BinOp::BitOr
                    | BinOp::BitXor
                    | BinOp::Shl
                    | BinOp::Shr
            );
            is_arithmetic
                && is_literal_expression(&binop.left)
                && is_literal_expression(&binop.right)
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => false,
    }
}

/// Lua's integer-to-float conversion: C's `(double)` cast, which rounds to
/// the nearest representable value.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "the C cast Lua itself uses, and i64 has no From into f64"
)]
pub(crate) const fn int_to_float(value: i64) -> f64 {
    value as f64
}

/// Wraps `expr` in span-less parentheses.
pub(crate) fn parenthesize(expr: Expression) -> Expression {
    Expression::Parenthesized(Box::new(ParenthesizedExpression { span: Span::default(), expr }))
}

/// Whether `inner` printed bare as the left operand of `outer` reparses as
/// that same operand.
pub(crate) fn can_unwrap_in_binop_lhs(inner: &Expression, outer: BinOp) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Var(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => true,
        // Unary ops bind looser than ^, so (-a)^b needs its parens.
        Expression::UnaryOp(_) => outer != BinOp::Pow,
        Expression::BinaryOp(binop) => binop.op.can_nest_bare(outer, OperandSide::Left),
        Expression::FunctionCall(_)
        | Expression::VarArg(_)
        | Expression::Parenthesized(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => false,
    }
}

/// Whether `inner` printed bare as the right operand of `outer` reparses as
/// that same operand.
pub(crate) fn can_unwrap_in_binop_rhs(inner: &Expression, outer: BinOp) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Var(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_)
        | Expression::UnaryOp(_) => true,
        Expression::BinaryOp(binop) => binop.op.can_nest_bare(outer, OperandSide::Right),
        Expression::FunctionCall(_)
        | Expression::VarArg(_)
        | Expression::Parenthesized(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => false,
    }
}
