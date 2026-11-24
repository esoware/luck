use luck_allocator::Allocator;
use luck_ast::{Expression, GetSpan as _, MAX_NESTING_DEPTH, expr_nesting_depth, is_multi_value};

/// How many levels of nesting a rewrite may add, so a run of rewrites can't
/// trade a flat program for one nested to the parser's cap.
pub(crate) const MAX_ADDED_DEPTH: usize = 32;

/// Whether `expr` still nests within the parser's cap once it sits
/// `added_levels` below `depth`, the level the walk has reached.
///
/// The output has to reparse, so a rewrite that moves an expression deeper
/// asks first.
pub(crate) fn fits_at_depth(depth: usize, added_levels: usize, expr: &Expression<'_>) -> bool {
    depth + added_levels + expr_nesting_depth(expr) <= MAX_NESTING_DEPTH
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
