//! Expression predicates and builders, and the integer-to-float conversion,
//! that the passes share.

use luck_ast::{
    BinaryOp, Expression, Field, Literal, ParenthesizedExpression, Var, ensure_sufficient_stack,
};
use luck_token::{BinOp, LuaNumber, NumberSubtypes, OperandSide, Span, UnOp, parse_lua_number};

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

/// What the operators that can raise care about in a literal operand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LiteralKind {
    /// A number with no known exact integer representation.
    Float,
    /// A number the bitwise operators accept.
    Integer,
    String,
    /// `nil`, a boolean, a Luau integer literal, or the result of `not`, an
    /// equality, or a comparison: no operator but `not`, `==`, `~=`, `and`, or
    /// `or` accepts one.
    Other,
}

impl LiteralKind {
    const fn is_number(self) -> bool {
        matches!(self, Self::Float | Self::Integer)
    }

    const fn is_concatenable(self) -> bool {
        matches!(self, Self::Float | Self::Integer | Self::String)
    }
}

/// The kind of a literal expression whose evaluation can't raise, or `None`
/// for anything else. Operators are checked against the operand kinds, since
/// `nil + 1` and `1 < "a"` raise with no variable in sight.
fn literal_kind(expr: &Expression) -> Option<LiteralKind> {
    match expr {
        Expression::Number(literal) => Some(number_literal_kind(literal)),
        Expression::StringLiteral(_) => Some(LiteralKind::String),
        Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Integer(_) => Some(LiteralKind::Other),
        Expression::Parenthesized(paren) => literal_kind(unparenthesized(&paren.expr)),
        Expression::UnaryOp(unop) => ensure_sufficient_stack(|| {
            let operand = literal_kind(&unop.operand)?;
            unary_literal_kind(unop.op, operand)
        }),
        Expression::BinaryOp(binop) => ensure_sufficient_stack(|| binary_literal_kind(binop)),
        // Luau: casts and type arguments are erased before evaluation.
        Expression::TypeCast(cast) => ensure_sufficient_stack(|| literal_kind(&cast.expr)),
        // Luau
        Expression::TypeInstantiation(instantiation) => {
            ensure_sufficient_stack(|| literal_kind(&instantiation.expr))
        }
        Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => None,
    }
}

fn number_literal_kind(literal: &Literal) -> LiteralKind {
    if matches!(parse_lua_number(&literal.text, NumberSubtypes::IntFloat), Some(LuaNumber::Int(_)))
    {
        LiteralKind::Integer
    } else {
        LiteralKind::Float
    }
}

const fn unary_literal_kind(op: UnOp, operand: LiteralKind) -> Option<LiteralKind> {
    match op {
        UnOp::Not => Some(LiteralKind::Other),
        UnOp::Neg => {
            if operand.is_number() {
                Some(operand)
            } else {
                None
            }
        }
        UnOp::Len => {
            if matches!(operand, LiteralKind::String) {
                Some(LiteralKind::Integer)
            } else {
                None
            }
        }
        UnOp::BitNot => {
            if matches!(operand, LiteralKind::Integer) {
                Some(LiteralKind::Integer)
            } else {
                None
            }
        }
    }
}

fn binary_literal_kind(binop: &BinaryOp) -> Option<LiteralKind> {
    let left = literal_kind(&binop.left)?;
    let right = literal_kind(&binop.right)?;
    let are_numbers = left.is_number() && right.is_number();
    let are_integers = left == LiteralKind::Integer && right == LiteralKind::Integer;
    let arithmetic_kind = if are_integers { LiteralKind::Integer } else { LiteralKind::Float };
    match binop.op {
        BinOp::And | BinOp::Or => Some(if left == right { left } else { LiteralKind::Other }),
        BinOp::Eq | BinOp::Ne => Some(LiteralKind::Other),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let are_strings = left == LiteralKind::String && right == LiteralKind::String;
            (are_numbers || are_strings).then_some(LiteralKind::Other)
        }
        BinOp::Concat => {
            (left.is_concatenable() && right.is_concatenable()).then_some(LiteralKind::String)
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul => are_numbers.then_some(arithmetic_kind),
        BinOp::Div | BinOp::Pow => are_numbers.then_some(LiteralKind::Float),
        // `%` and `//` raise on a zero integer divisor in 5.3+.
        BinOp::Mod | BinOp::FloorDiv => {
            (are_numbers && is_nonzero_number_literal(&binop.right)).then_some(arithmetic_kind)
        }
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
            are_integers.then_some(LiteralKind::Integer)
        }
    }
}

fn is_nonzero_number_literal(expr: &Expression) -> bool {
    let Expression::Number(literal) = unparenthesized(expr) else {
        return false;
    };
    match parse_lua_number(&literal.text, NumberSubtypes::IntFloat) {
        Some(LuaNumber::Int(value)) => value != 0,
        Some(LuaNumber::Float(value)) => value != 0.0f64,
        None => false,
    }
}

/// Whether evaluating `expr` can't raise or call a metamethod.
///
/// A name read may be a global's, which fires `__index` on a metatabled
/// environment, so only literals, closures, `...`, and tables built from them
/// qualify.
pub(crate) fn is_pure_expr(expr: &Expression) -> bool {
    is_effect_free(expr, false)
}

/// [`is_pure_expr`], except that a name read counts: for a caller that keeps
/// every read in its original order, so `__index` can't observe a change.
pub(crate) fn is_pure_modulo_name_reads(expr: &Expression) -> bool {
    is_effect_free(expr, true)
}

fn is_effect_free(expr: &Expression, can_read_names: bool) -> bool {
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
        Expression::Parenthesized(paren) => {
            is_effect_free(unparenthesized(&paren.expr), can_read_names)
        }
        Expression::UnaryOp(_) => literal_kind(expr).is_some(),
        Expression::BinaryOp(binop) => {
            if matches!(binop.op, BinOp::And | BinOp::Or) {
                // and/or never invoke metamethods, so they are pure whenever
                // both operands are.
                ensure_sufficient_stack(|| {
                    is_effect_free(&binop.left, can_read_names)
                        && is_effect_free(&binop.right, can_read_names)
                })
            } else {
                literal_kind(expr).is_some()
            }
        }
        Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
            Field::Bracketed { key, value, .. } => {
                is_valid_table_key(key) && is_effect_free(value, can_read_names)
            }
            Field::Named { value, .. } | Field::Positional { value, .. } => {
                is_effect_free(value, can_read_names)
            }
        }),
        Expression::Var(var) => match var {
            Var::Name(_) => can_read_names,
            // Field access can raise on a nil prefix or fire __index, just
            // like `Var::Index`.
            Var::FieldAccess(_) | Var::Index(_) => false,
        },
        // Luau type casts are transparent wrappers, so purity is the inner
        // expression's.
        Expression::TypeCast(cast) => {
            ensure_sufficient_stack(|| is_effect_free(&cast.expr, can_read_names))
        }
        // Luau: explicit type arguments aren't evaluated at runtime.
        Expression::TypeInstantiation(instantiation) => {
            ensure_sufficient_stack(|| is_effect_free(&instantiation.expr, can_read_names))
        }
        Expression::FunctionCall(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => false,
    }
}

/// Whether `key` is a table constructor `[key]` that can't raise: `nil` and
/// NaN raise, and only a key spelled out as a literal is known to be neither.
fn is_valid_table_key(key: &Expression) -> bool {
    match unparenthesized(key) {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::UnaryOp(unop) => {
            unop.op == UnOp::Neg && matches!(unparenthesized(&unop.operand), Expression::Number(_))
        }
        Expression::Parenthesized(_)
        | Expression::Nil(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => false,
    }
}

/// Whether `expr` is a literal, or operators over literals, that can't raise:
/// neither its value nor its evaluation depends on where it runs, so a pass
/// can move it.
pub(crate) fn is_closed_literal_expr(expr: &Expression) -> bool {
    literal_kind(expr).is_some()
}

/// Whether `expr` evaluates into exactly one register with no temporaries.
pub(crate) const fn is_single_register_value(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_) => true,
        Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::Parenthesized(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
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
        Expression::UnaryOp(_) | Expression::BinaryOp(_) => {
            literal_kind(expr).is_some_and(|kind| kind.is_number() || kind == LiteralKind::String)
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
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

/// Whether a binary expression on `op`, printed bare under a unary operator,
/// regroups. Every binary operator but `^` binds looser than a unary one.
pub(crate) const fn binds_looser_than_unary(op: BinOp) -> bool {
    op.precedence().0 < UnOp::PRECEDENCE
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
