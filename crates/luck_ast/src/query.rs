use luck_token::Span;

use crate::expr::{Expression, Var};
use crate::stmt::Statement;

/// A `Name(.Name)*` chain's names, root first, and where its first and last
/// names sit.
#[derive(Debug)]
pub struct DottedPath<'ast> {
    pub segments: Vec<&'ast str>,
    pub root_span: Span,
    pub leaf_span: Span,
}

/// Any other link, such as indexing or a call, yields `None`.
#[must_use]
pub fn dotted_path(var: &Var) -> Option<DottedPath<'_>> {
    let leaf_span = match var {
        Var::Name(token) => token.span,
        Var::FieldAccess(field_access) => field_access.name.span,
        Var::Index(_) => return None,
    };
    let mut segments: Vec<&str> = Vec::new();
    let mut cursor = var;
    let root_span = loop {
        match cursor {
            Var::Name(token) => {
                segments.push(token.identifier()?);
                break token.span;
            }
            Var::FieldAccess(field_access) => {
                segments.push(field_access.name.identifier()?);
                let Expression::Var(prefix) = &field_access.prefix else {
                    return None;
                };
                cursor = prefix;
            }
            Var::Index(_) => return None,
        }
    };
    segments.reverse();
    Some(DottedPath { segments, root_span, leaf_span })
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
pub const fn is_multi_value(expr: &Expression) -> bool {
    match expr {
        Expression::FunctionCall(_) | Expression::VarArg(_) => true,
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::Parenthesized(_)
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

/// Whether `expr` may stand bare in a prefix position: a call callee, a
/// method receiver, an index or field prefix, or an instantiated expression.
///
/// Anything else needs parentheses there, as in `("s"):rep(2)` or `({}).x`.
#[must_use]
pub const fn is_prefix_expr(expr: &Expression) -> bool {
    match expr {
        Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::Parenthesized(_)
        | Expression::TypeInstantiation(_) => true,
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
        Statement::FunctionCall(call) => expr_starts_with_paren(&call.callee),
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
            Expression::FunctionCall(call) => &call.callee,
            Expression::Var(Var::FieldAccess(field_access)) => &field_access.prefix,
            Expression::Var(Var::Index(index)) => &index.prefix,
            Expression::BinaryOp(binary_op) => &binary_op.left,
            // Luau
            Expression::TypeCast(cast) => &cast.expr,
            // Luau
            Expression::TypeInstantiation(instantiation) => &instantiation.expr,
            Expression::Var(Var::Name(_))
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
        Var::FieldAccess(field_access) => expr_starts_with_paren(&field_access.prefix),
        Var::Index(index) => expr_starts_with_paren(&index.prefix),
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
            Expression::FunctionCall(call) => &call.callee,
            Expression::BinaryOp(binary_op) => &binary_op.left,
            // Luau
            Expression::TypeCast(cast) => &cast.expr,
            Expression::Var(Var::FieldAccess(field_access)) => &field_access.prefix,
            Expression::Var(Var::Index(index)) => &index.prefix,
            Expression::Var(Var::Name(_))
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
            | Expression::TypeInstantiation(_)
            | Expression::Error(_) => return false,
        };
    }
}
