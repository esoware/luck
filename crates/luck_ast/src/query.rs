//! Read-only structural queries shared by the printers.

use crate::expr::Var;
use crate::{Expression, Statement};

/// Whether a reprinted statement's first token is `(`. Both printers must
/// guard these statements: after a call, `(...)` glues onto the previous
/// expression and re-parses as a chained call - a silent semantics change.
pub fn stmt_starts_with_paren(stmt: &Statement) -> bool {
    match stmt {
        Statement::FunctionCall(call) => expr_starts_with_paren(&call.call.callee),
        Statement::Assignment(assign) => {
            let first_var = assign.targets.first();
            first_var.is_some_and(var_starts_with_paren)
        }
        Statement::CompoundAssignment(compound) => var_starts_with_paren(&compound.var),
        _ => false,
    }
}

pub fn expr_starts_with_paren(expr: &Expression) -> bool {
    match expr {
        Expression::Parenthesized(_) => true,
        Expression::FunctionCall(call) => expr_starts_with_paren(&call.callee),
        Expression::Var(var) => var_starts_with_paren(var),
        Expression::BinaryOp(binop) => expr_starts_with_paren(&binop.left),
        Expression::UnaryOp(_) => false,
        Expression::TypeCast(cast) => expr_starts_with_paren(&cast.expr),
        _ => false,
    }
}

pub fn var_starts_with_paren(var: &Var) -> bool {
    match var {
        Var::Name(_) => false,
        Var::FieldAccess(field_access) => expr_starts_with_paren(&field_access.prefix),
        Var::Index(index) => expr_starts_with_paren(&index.prefix),
    }
}

/// Whether a reprinted expression's first token is `{`. Inside an
/// interpolated string, `{` directly after the interpolation opener
/// forms `{{`, which Luau rejects - printers must separate them.
pub fn expr_starts_with_brace(expr: &Expression) -> bool {
    match expr {
        Expression::TableConstructor(_) => true,
        Expression::FunctionCall(call) => expr_starts_with_brace(&call.callee),
        Expression::BinaryOp(binop) => expr_starts_with_brace(&binop.left),
        Expression::TypeCast(cast) => expr_starts_with_brace(&cast.expr),
        Expression::Var(var) => match var.as_ref() {
            Var::Name(_) => false,
            Var::FieldAccess(field_access) => expr_starts_with_brace(&field_access.prefix),
            Var::Index(index) => expr_starts_with_brace(&index.prefix),
        },
        _ => false,
    }
}
