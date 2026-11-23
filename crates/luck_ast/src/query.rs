use crate::expr::{Expression, Var};
use crate::stmt::Statement;

/// `expr` inside every [`Expression::Parenthesized`] around it, for a place
/// where taking one value, or reading a Luau integer as a value, changes
/// nothing.
#[must_use]
pub fn unwrap_parentheses<'tree, 'ast>(expr: &'tree Expression<'ast>) -> &'tree Expression<'ast> {
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
pub fn is_multi_value(expr: &Expression<'_>) -> bool {
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

/// The prefix a call or an assignment starts with, inside any chain it
/// starts.
///
/// A printer that writes it in parentheses starts the statement with `(`,
/// which glues onto a statement before it that ends in an expression and
/// reads back as a call, so it puts a `;` between them.
#[must_use]
pub fn leading_prefix<'tree, 'ast>(
    stmt: &'tree Statement<'ast>,
) -> Option<&'tree Expression<'ast>> {
    let mut current = match stmt {
        Statement::FunctionCall(chain) => &chain.prefix,
        Statement::Assignment(assignment) => match assignment.targets.first()? {
            Var::Name(_) => return None,
            Var::Suffixed(chain) => &chain.prefix,
        },
        // Luau
        Statement::CompoundAssignment(compound) => match &compound.var {
            Var::Name(_) => return None,
            Var::Suffixed(chain) => &chain.prefix,
        },
        Statement::DoBlock(_)
        | Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::IfStatement(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::LocalAssignment(_)
        | Statement::Return(_)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => return None,
    };
    while let Expression::Suffixed(chain) = current {
        current = &chain.prefix;
    }
    Some(current)
}
