//! Expression predicates and builders, and the integer-to-float conversion,
//! that the passes share.

use luck_ast::{Expression, Field, ParenthesizedExpression, Var, ensure_sufficient_stack};
use luck_token::{BinOp, OperandSide, Span, UnOp};

/// `expr` with any enclosing parentheses stripped.
pub(crate) fn unparenthesized(expr: &Expression) -> &Expression {
    let mut current = expr;
    while let Expression::Parenthesized(paren) = current {
        current = &paren.expr;
    }
    current
}

pub(crate) fn extract_boolean(expr: &Expression) -> Option<bool> {
    match unparenthesized(expr) {
        Expression::True(_) => Some(true),
        Expression::False(_) => Some(false),
        Expression::Parenthesized(_)
        | Expression::Nil(_)
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
    match unparenthesized(expr) {
        Expression::Nil(_) => true,
        Expression::Parenthesized(_)
        | Expression::False(_)
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
fn is_literal_expr(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::Parenthesized(paren) => is_literal_expr(unparenthesized(&paren.expr)),
        Expression::UnaryOp(unop) => ensure_sufficient_stack(|| is_literal_expr(&unop.operand)),
        Expression::BinaryOp(binop) => ensure_sufficient_stack(|| {
            is_literal_expr(&binop.left) && is_literal_expr(&binop.right)
        }),
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
pub(crate) fn is_pure_expr(expr: &Expression) -> bool {
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
        Expression::Parenthesized(paren) => is_pure_expr(unparenthesized(&paren.expr)),
        Expression::UnaryOp(_) => is_literal_expr(expr),
        Expression::BinaryOp(binop) => {
            if matches!(binop.op, BinOp::And | BinOp::Or) {
                // and/or never invoke metamethods, so they are pure whenever
                // both operands are.
                ensure_sufficient_stack(|| is_pure_expr(&binop.left) && is_pure_expr(&binop.right))
            } else {
                is_literal_expr(expr)
            }
        }
        Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
            Field::Bracketed { key, value, .. } => {
                is_pure_expr(key) && is_pure_expr(value)
            }
            Field::Named { value, .. } | Field::Positional { value, .. } => {
                is_pure_expr(value)
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
        Expression::TypeCast(cast) => ensure_sufficient_stack(|| is_pure_expr(&cast.expr)),
        // Luau: explicit type arguments aren't evaluated at runtime.
        Expression::TypeInstantiation(instantiation) => {
            ensure_sufficient_stack(|| is_pure_expr(&instantiation.expr))
        }
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
        Expression::Parenthesized(paren) => is_always_truthy(unparenthesized(&paren.expr)),
        Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Neg) => {
            is_literal_expr(&unop.operand) && is_always_truthy(&unop.operand)
        }
        Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Len) => is_literal_expr(&unop.operand),
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
            is_arithmetic && is_literal_expr(&binop.left) && is_literal_expr(&binop.right)
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

/// Whether `expr` prints a Luau if-expression last, whose `else` branch would
/// take in an operator printed after it.
pub(crate) fn ends_in_if_expr(expr: &Expression) -> bool {
    let mut current = expr;
    loop {
        current = match current {
            Expression::IfExpression(_) => return true,
            Expression::BinaryOp(binop) => &binop.right,
            Expression::UnaryOp(unop) => &unop.operand,
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => return false,
        };
    }
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
        Expression::UnaryOp(_) => outer != BinOp::Pow && !ends_in_if_expr(inner),
        Expression::BinaryOp(binop) => {
            binop.op.can_nest_bare(outer, OperandSide::Left) && !ends_in_if_expr(inner)
        }
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
        | Expression::FunctionDef(_) => true,
        Expression::UnaryOp(_) => !ends_in_if_expr(inner),
        Expression::BinaryOp(binop) => {
            binop.op.can_nest_bare(outer, OperandSide::Right) && !ends_in_if_expr(inner)
        }
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
