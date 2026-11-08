//! Function statements and function expressions as one view, for the node
//! rules that check every function.

use luck_ast::{Expression, FunctionBody, Statement};
use luck_token::Span;

use crate::node_type::{NodeType, NodeTypeSet};

/// A `function`, `local function`, or `global function` statement, or a
/// function expression.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FunctionNode<'ast> {
    pub body: &'ast FunctionBody,
    /// The whole statement or expression.
    pub span: Span,
    /// The declared name, such as `a.b:c`. `None` for a function expression.
    pub name_span: Option<Span>,
    /// Whether it's declared as `function a:b()`, with an implicit `self`.
    pub is_method: bool,
}

impl<'ast> FunctionNode<'ast> {
    /// The node types that hold a function.
    pub(crate) const NODE_TYPES: NodeTypeSet = NodeTypeSet::from_types(&[
        NodeType::FunctionDecl,
        NodeType::LocalFunction,
        NodeType::GlobalFunction,
        NodeType::FunctionDef,
    ]);

    pub(crate) fn of_statement(stmt: &'ast Statement) -> Option<Self> {
        let (body, span, name_span, is_method) = match stmt {
            Statement::FunctionDecl(decl) => {
                (&decl.body, decl.span, decl.name.span, decl.name.method.is_some())
            }
            Statement::LocalFunction(local_function) => {
                (&local_function.body, local_function.span, local_function.name.span, false)
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                (&global_function.body, global_function.span, global_function.name.span, false)
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => return None,
        };
        Some(Self { body, span, name_span: Some(name_span), is_method })
    }

    pub(crate) const fn of_expression(expr: &'ast Expression) -> Option<Self> {
        let Expression::FunctionDef(function) = expr else {
            return None;
        };
        Some(Self { body: &function.body, span: function.span, name_span: None, is_method: false })
    }
}
