use core::iter;

use luck_allocator::Allocator;
use luck_ast::{
    BinaryOp, Expression, Field, GetSpan as _, MAX_NESTING_DEPTH, Suffix, expr_nesting_depth,
    is_multi_value,
};
use luck_token::{Associativity, BinOp, LuaNumber, Span, UnOp};

pub(crate) const fn extract_boolean(expr: &Expression<'_>) -> Option<bool> {
    match expr {
        Expression::True(_) => Some(true),
        Expression::False(_) => Some(false),
        Expression::Parenthesized(_)
        | Expression::Nil(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => None,
    }
}

pub(crate) const fn is_nil(expr: &Expression<'_>) -> bool {
    matches!(expr, Expression::Nil(_))
}

/// `Some(false)` for `nil` and `false`, `Some(true)` for `true` and a
/// number or string literal, the opposite for `not` of one, and `None` for
/// anything else.
pub(crate) fn constant_truthiness(expr: &Expression<'_>) -> Option<bool> {
    match expr {
        Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_) => Some(true),
        Expression::False(_) | Expression::Nil(_) => Some(false),
        Expression::UnaryOp(unop) if unop.op == UnOp::Not => {
            constant_truthiness(&unop.operand).map(|is_truthy| !is_truthy)
        }
        // Truncating to the first value keeps that value's truth.
        Expression::Parenthesized(paren) => constant_truthiness(&paren.expr),
        Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => None,
    }
}

/// What the operators that can raise care about in a literal operand.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiteralKind {
    /// A float, which the bitwise operators may refuse.
    Float,
    /// A Lua 5.3+ integer, which the bitwise operators accept.
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
pub(crate) fn literal_kind(expr: &Expression<'_>) -> Option<LiteralKind> {
    match expr {
        Expression::Number(literal) => Some(match literal.value {
            LuaNumber::Int(_) => LiteralKind::Integer,
            LuaNumber::Float(_) => LiteralKind::Float,
        }),
        Expression::StringLiteral(_) => Some(LiteralKind::String),
        Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Integer(_) => Some(LiteralKind::Other),
        Expression::Parenthesized(paren) => literal_kind(&paren.expr),
        Expression::UnaryOp(unop) => {
            let operand = literal_kind(&unop.operand)?;
            unary_literal_kind(unop.op, operand)
        }
        Expression::BinaryOp(binary_op) => run_literal_kind(binary_op),
        // Luau: a cast is erased before evaluation.
        Expression::TypeCast(cast) => literal_kind(&cast.expr),
        Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => None,
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

/// Each operator applies in the order the level groups them.
fn run_literal_kind(run: &BinaryOp<'_>) -> Option<LiteralKind> {
    match run.precedence().1 {
        Associativity::Left => {
            let mut kind = literal_kind(&run.first)?;
            for link in &run.links {
                let operand = literal_kind(&link.operand)?;
                let is_nonzero = is_nonzero_number_literal(&link.operand);
                kind = binary_literal_kind(kind, link.op, operand, is_nonzero)?;
            }
            Some(kind)
        }
        Associativity::Right => {
            let (last, before_last) = run.links.split_last()?;
            let mut kind = literal_kind(&last.operand)?;
            let left_operands =
                before_last.iter().rev().map(|link| &link.operand).chain(iter::once(&run.first));
            for (link, left_operand) in run.links.iter().rev().zip(left_operands) {
                let is_nonzero = is_nonzero_number_literal(&link.operand);
                kind = binary_literal_kind(literal_kind(left_operand)?, link.op, kind, is_nonzero)?;
            }
            Some(kind)
        }
    }
}

/// `is_rhs_nonzero` says whether the right operand as written, which for a
/// right-associative operator is the start of the rest of its run, is a
/// nonzero number literal.
fn binary_literal_kind(
    left: LiteralKind,
    op: BinOp,
    right: LiteralKind,
    is_rhs_nonzero: bool,
) -> Option<LiteralKind> {
    let are_numbers = left.is_number() && right.is_number();
    let are_integers = left == LiteralKind::Integer && right == LiteralKind::Integer;
    let arithmetic_kind = if are_integers { LiteralKind::Integer } else { LiteralKind::Float };
    match op {
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
        BinOp::Mod | BinOp::FloorDiv => (are_numbers && is_rhs_nonzero).then_some(arithmetic_kind),
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
            are_integers.then_some(LiteralKind::Integer)
        }
    }
}

fn is_nonzero_number_literal(expr: &Expression<'_>) -> bool {
    let Expression::Number(literal) = expr else {
        return false;
    };
    literal.value.to_f64() != 0.0f64
}

/// A name read may be a global's, which fires `__index` on a metatabled
/// environment, so only literals, closures, `...`, and tables built from them
/// qualify.
pub(crate) fn is_pure_expr(expr: &Expression<'_>) -> bool {
    is_effect_free(expr, false)
}

/// [`is_pure_expr`], except that a name read counts: for a caller that keeps
/// every read in its original order, so `__index` can't observe a change.
pub(crate) fn is_pure_modulo_name_reads(expr: &Expression<'_>) -> bool {
    is_effect_free(expr, true)
}

fn is_effect_free(expr: &Expression<'_>, can_read_names: bool) -> bool {
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
        Expression::Parenthesized(paren) => is_effect_free(&paren.expr, can_read_names),
        Expression::UnaryOp(_) => literal_kind(expr).is_some(),
        Expression::BinaryOp(binary_op) => {
            if is_logical(binary_op) {
                // and/or never invoke metamethods, so they are pure whenever
                // every operand is.
                is_effect_free(&binary_op.first, can_read_names)
                    && binary_op
                        .links
                        .iter()
                        .all(|link| is_effect_free(&link.operand, can_read_names))
            } else {
                literal_kind(expr).is_some()
            }
        }
        Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
            Field::Bracketed { key, value, .. } => {
                is_valid_table_key(key) && is_effect_free(value, can_read_names)
            }
            Field::Named { value, .. } | Field::Positional(value) => {
                is_effect_free(value, can_read_names)
            }
        }),
        Expression::Name(_) => can_read_names,
        // A field access or an index can raise on a nil prefix or fire
        // `__index`, and a call can do anything. Luau's explicit type
        // arguments aren't evaluated at runtime.
        Expression::Suffixed(chain) => {
            chain.suffixes.iter().all(|suffix| matches!(suffix, Suffix::TypeInstantiation { .. }))
                && is_effect_free(&chain.prefix, can_read_names)
        }
        // Luau type casts are transparent wrappers, so purity is the inner
        // expression's.
        Expression::TypeCast(cast) => is_effect_free(&cast.expr, can_read_names),
        Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => false,
    }
}

/// Whether `key` is a table constructor `[key]` that can't raise: `nil` and
/// NaN raise, and only a key spelled out as a literal is known to be neither.
fn is_valid_table_key(key: &Expression<'_>) -> bool {
    match key {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::UnaryOp(unop) => {
            unop.op == UnOp::Neg && matches!(unop.operand, Expression::Number(_))
        }
        Expression::Parenthesized(paren) => is_valid_table_key(&paren.expr),
        Expression::Nil(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// How many levels of nesting a rewrite may add, so a run of rewrites can't
/// trade a flat program for one nested to the parser's cap.
pub(crate) const MAX_ADDED_DEPTH: usize = 32;

/// Whether `expr` still nests within the parser's cap once it sits
/// `added_levels` below `depth`, the level the walk has reached.
///
/// The output has to reparse, so a rewrite that moves an expression deeper
/// asks first.
pub(crate) fn fits_at_depth(depth: usize, added_levels: usize, expr: &Expression<'_>) -> bool {
    depth.saturating_add(added_levels).saturating_add(expr_nesting_depth(expr)) <= MAX_NESTING_DEPTH
}

/// Whether `expr` evaluates into exactly one register with no temporaries.
pub(crate) const fn is_single_register_value(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_) => true,
        Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// Whether the expression is guaranteed truthy at runtime. Only `nil` and
/// `false` are falsy in Lua, so numbers, strings, tables, and functions pass.
pub(crate) fn is_always_truthy(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::FunctionDef(_)
        | Expression::TableConstructor(_)
        | Expression::True(_) => true,
        Expression::Parenthesized(paren) => is_always_truthy(&paren.expr),
        Expression::UnaryOp(_) | Expression::BinaryOp(_) => {
            literal_kind(expr).is_some_and(|kind| kind.is_number() || kind == LiteralKind::String)
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

pub(crate) fn parenthesize<'ast>(
    allocator: &'ast Allocator,
    expr: Expression<'ast>,
) -> Expression<'ast> {
    let span = expr.span();
    Expression::new_parenthesized(span, expr, allocator)
}

/// Parenthesizes a call or `...` that replaces an `and`/`or`, which always
/// yields one value.
pub(crate) fn truncate_multi_value<'ast>(
    allocator: &'ast Allocator,
    expr: Expression<'ast>,
) -> Expression<'ast> {
    if is_multi_value(&expr) { parenthesize(allocator, expr) } else { expr }
}

pub(crate) const fn boolean<'ast>(value: bool, span: Span) -> Expression<'ast> {
    if value { Expression::True(span) } else { Expression::False(span) }
}

/// Whether `run` is `and` or `or`, which no metamethod can intercept.
pub(crate) fn is_logical(run: &BinaryOp<'_>) -> bool {
    matches!(run.links.first().op, BinOp::And | BinOp::Or)
}
