use luck_token::Span;

use crate::expr::{Expression, Var};
use crate::stmt::Statement;

/// A `Name(.Name)*` chain's names, root first, and where its first and last
/// names sit. [`ChainHead::dotted_path`](crate::ChainHead::dotted_path)
/// finds one.
#[derive(Debug)]
pub struct DottedPath<'ast> {
    pub segments: Vec<&'ast str>,
    pub root_span: Span,
    pub leaf_span: Span,
}

#[must_use]
pub fn unwrap_parentheses(expr: &Expression) -> &Expression {
    let mut current = expr;
    while let Expression::Parenthesized(parenthesized) = current {
        current = &parenthesized.expr;
    }
    current
}

/// Whether `expr` is a call or `...`, the only forms that can yield any
/// number of values, none included.
///
/// Parenthesizing one, or moving it out of the last position of a list,
/// truncates it to exactly one value.
#[must_use]
pub fn is_multi_value(expr: &Expression) -> bool {
    match expr {
        Expression::VarArg(_) => true,
        Expression::Suffixed(chain) => chain.is_call(),
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// Whether `expr` may stand bare in a prefix position: before a field, an
/// index, a call, or a Luau instantiation.
///
/// Anything else needs parentheses there, as in `("s"):rep(2)` or `({}).x`.
#[must_use]
pub const fn is_prefix_expr(expr: &Expression) -> bool {
    match expr {
        Expression::Name(_) | Expression::Suffixed(_) | Expression::Parenthesized(_) => true,
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// Both printers must guard a statement starting with `(`. Printed after a statement that
/// ends in an expression, `(...)` glues onto that expression and re-parses
/// as a call, silently changing the meaning.
#[must_use]
pub fn stmt_starts_with_paren(stmt: &Statement) -> bool {
    match stmt {
        Statement::FunctionCall(chain) => expr_starts_with_paren(&chain.prefix),
        Statement::Assignment(assignment) => {
            let first_var = assignment.targets.first();
            first_var.is_some_and(var_starts_with_paren)
        }
        // Luau
        Statement::CompoundAssignment(compound) => var_starts_with_paren(&compound.var),
        Statement::DoBlock(_)
        | Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::IfStatement(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::LocalAssignment(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    }
}

fn expr_starts_with_paren(expr: &Expression) -> bool {
    let mut current = expr;
    loop {
        current = match current {
            Expression::Parenthesized(_) => return true,
            Expression::Suffixed(chain) => &chain.prefix,
            Expression::BinaryOp(binary_op) => &binary_op.first,
            // Luau
            Expression::TypeCast(cast) => &cast.expr,
            Expression::Name(_)
            | Expression::UnaryOp(_)
            | Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_) => return false,
        };
    }
}

fn var_starts_with_paren(var: &Var) -> bool {
    match var {
        Var::Name(_) => false,
        Var::Suffixed(chain) => expr_starts_with_paren(&chain.prefix),
    }
}

/// Inside an interpolated string, `{` directly after the interpolation
/// opener forms `{{`, which Luau rejects, so printers must separate them.
#[must_use]
pub fn expr_starts_with_brace(expr: &Expression) -> bool {
    let mut current = expr;
    loop {
        current = match current {
            Expression::TableConstructor(_) => return true,
            Expression::Suffixed(chain) => &chain.prefix,
            Expression::BinaryOp(binary_op) => &binary_op.first,
            // Luau
            Expression::TypeCast(cast) => &cast.expr,
            Expression::Name(_)
            | Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Parenthesized(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_) => return false,
        };
    }
}
