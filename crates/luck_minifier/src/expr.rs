use luck_allocator::Allocator;
use luck_ast::{Expression, GetSpan as _, MAX_NESTING_DEPTH, expr_nesting_depth};

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

pub(crate) fn parenthesize<'ast>(
    allocator: &'ast Allocator,
    expr: Expression<'ast>,
) -> Expression<'ast> {
    let span = expr.span();
    Expression::new_parenthesized(span, expr, allocator)
}

/// Parenthesizes a call or `...` that replaces an expression that always
/// yields one value, an `and`/`or` or a Luau cast.
pub(crate) fn truncate_multi_value<'ast>(
    allocator: &'ast Allocator,
    expr: Expression<'ast>,
) -> Expression<'ast> {
    if expr.is_multi_value() { parenthesize(allocator, expr) } else { expr }
}
