use luck_ast::{AstKind, AstType, FunctionBody};
use luck_semantic::{AstNode, AstNodes, AstTypesBitset};
use luck_token::Span;

/// A `function`, `local function`, `global function`, or `type function`
/// statement, or a function expression.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FunctionNode<'ast> {
    pub body: &'ast FunctionBody<'ast>,
    /// The whole statement or expression.
    pub span: Span,
    /// The declared name, such as `a.b:c`. `None` for a function expression.
    pub name_span: Option<Span>,
    pub is_method: bool,
}

impl<'ast> FunctionNode<'ast> {
    /// Every function has one body, whose node finds it.
    pub(crate) const NODE_TYPES: AstTypesBitset =
        AstTypesBitset::from_types(&[AstType::FunctionBody]);

    /// The function whose body is `node`.
    pub(crate) fn of(node: &AstNode<'ast>, nodes: &AstNodes<'ast>) -> Option<Self> {
        let body = node.kind().as_function_body()?;
        let parent = nodes.parent_kind(node.id())?;
        let (span, name_span, is_method) = if let AstKind::FunctionDecl(decl) = parent {
            (decl.span, Some(decl.name.span), decl.name.method.is_some())
        } else if let AstKind::LocalFunction(local_function) = parent {
            (local_function.span, Some(local_function.name.span), false)
        } else if let AstKind::GlobalFunction(global_function) = parent {
            // Lua 5.5
            (global_function.span, Some(global_function.name.span), false)
        } else if let AstKind::TypeDeclaration(type_declaration) = parent {
            // Luau
            (type_declaration.span, Some(type_declaration.name.span), false)
        } else {
            (parent.as_function_def()?.span, None, false)
        };
        Some(Self { body, span, name_span, is_method })
    }
}
