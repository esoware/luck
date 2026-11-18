use luck_allocator::{Box, List};
use luck_ast_macros::Ast;
use luck_token::Span;

use crate::expr::Expression;
use crate::leaf::Name;
use crate::stmt::Block;
use crate::types::{GenericTypeList, Type};

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct FunctionBody<'ast> {
    pub span: Span,
    pub generics: Option<Box<'ast, GenericTypeList<'ast>>>,
    pub params: List<'ast, Parameter<'ast>>,
    pub vararg: Option<Box<'ast, VarArgParam<'ast>>>,
    pub return_type: Option<Type<'ast>>,
    pub block: Block<'ast>,
}

impl FunctionBody<'_> {
    /// Span of the closing `end` keyword: the last three bytes of the body
    /// span.
    ///
    /// Only meaningful for parsed ASTs, since synthetic bodies carry point
    /// spans. Callers use it only for source-anchored trivia, which
    /// synthetic ASTs don't have.
    #[must_use]
    pub const fn end_keyword_span(&self) -> Span {
        Span::new(self.span.end.saturating_sub(3), self.span.end)
    }
}

/// A typed name: function parameter or generic-for loop binding. It spans
/// from its name, so it holds no span of its own.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct Parameter<'ast> {
    pub name: Name<'ast>,
    pub type_annotation: Option<Type<'ast>>,
}

/// `...`, where `span` starts at the dots.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct VarArgParam<'ast> {
    pub span: Span,
    /// Lua 5.5: `...name`.
    pub name: Option<Name<'ast>>,
    /// Luau: `: T`, which may be a generic pack (`T...`).
    pub type_annotation: Option<Type<'ast>>,
}

/// Output must keep these, since some change runtime behavior (`@native`
/// forces native codegen).
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct FunctionAttribute<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
    /// Literal arguments of the bracketed `@[name(...)]` form; `None` for
    /// the plain `@name` form and for `@[name]` without arguments.
    #[visit(level)]
    pub args: Option<List<'ast, Expression<'ast>>>,
}
