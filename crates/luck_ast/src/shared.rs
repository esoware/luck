use luck_token::{Span, Token};

use crate::expr::Expression;
use crate::stmt::{LastStatement, Statement};
use crate::types::{GenericTypeList, Type};

/// A sequence of items separated by fixed tokens (typically commas).
///
/// Interior separators are implied by position; `has_trailing_separator`
/// records one after the final item (a trailing comma in a table
/// constructor, or a dangling separator preserved from parse recovery).
/// Separator spelling is implied by context, so no spans are stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Punctuated<T> {
    pub items: Vec<T>,
    pub has_trailing_separator: bool,
}

/// Function body: params + optional return type + block, closed by `end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionBody {
    pub span: Span,
    /// Luau: `<T, U...>` generic list before the parameter parens.
    pub generics: Option<Box<GenericTypeList>>,
    pub params: Punctuated<Parameter>,
    pub vararg: Option<VarArgParam>,
    /// Luau: `: T` return annotation after `)`.
    pub return_type: Option<Type>,
    pub block: Block,
}

impl FunctionBody {
    /// Span of the closing `end` keyword, derived from the body span,
    /// which always ends with it. Synthetic bodies carry point spans, so
    /// the result is meaningful only for parsed ASTs; callers consult it
    /// solely for source-anchored trivia, which synthetic ASTs lack.
    #[must_use]
    pub fn end_keyword_span(&self) -> Span {
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
    /// Luau: `: T`. The type may be a pack (`...number`).
    pub type_annotation: Option<Type>,
}

/// Block of statements with optional last statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub span: Span,
    pub stmts: Vec<Statement>,
    pub last_stmt: Option<Box<LastStatement>>,
}

/// A table field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Bracketed {
        span: Span,
        key: Expression,
        value: Expression,
    },
    Named {
        span: Span,
        name: Token,
        value: Expression,
    },
    Positional {
        span: Span,
        value: Expression,
    },
}
