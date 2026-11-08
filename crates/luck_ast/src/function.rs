use luck_token::{Span, Token};

use crate::expr::Expression;
use crate::punctuated::Punctuated;
use crate::stmt::Block;
use crate::types::{GenericTypeList, Type};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionBody {
    pub span: Span,
    /// Luau: `<T, U...>` generic list before the parameter parens.
    pub generics: Option<Box<GenericTypeList>>,
    pub params: Punctuated<Parameter>,
    pub vararg: Option<Box<VarArgParam>>,
    /// Luau: `: T` return annotation after `)`.
    pub return_type: Option<Type>,
    pub block: Block,
}

impl FunctionBody {
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

/// A typed name: function parameter or generic-for loop binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub span: Span,
    pub name: Token,
    /// Luau: `: T`.
    pub type_annotation: Option<Type>,
}

/// Vararg parameter (`...` or `...name` in Lua 5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarArgParam {
    pub span: Span,
    pub name: Option<Token>,
    /// Luau: `: T`, which may be a generic pack (`T...`).
    pub type_annotation: Option<Type>,
}

/// Luau function attribute: `@native`, `@checked`, `@deprecated`, and so on.
///
/// Output must keep these, since some change runtime behavior (`@native`
/// forces native codegen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionAttribute {
    pub span: Span,
    pub name: Token,
    /// Literal arguments of the bracketed `@[name(...)]` form; `None` for
    /// the plain `@name` form and for `@[name]` without arguments.
    pub args: Option<Punctuated<Expression>>,
}
