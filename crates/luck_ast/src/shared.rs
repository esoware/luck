use luck_token::{Span, Token};

use crate::expr::Expression;
use crate::stmt::{LastStatement, Statement};
use crate::types::{GenericTypeList, Type};

/// A sequence of items separated by fixed tokens (typically commas).
///
/// Each item carries the SPAN of its following separator; the final
/// item's is `None` (a `Some` on the final item preserves a dangling
/// separator from parse recovery). Same shape as table-constructor
/// fields. Separator spelling is implied by context, so only the span
/// is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Punctuated<T> {
    pub items: Vec<(T, Option<Span>)>,
}

/// Paired delimiter spans (parens, brackets, braces, angles). The
/// delimiter spelling is implied by the owning node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainedSpan {
    pub open: Span,
    pub close: Span,
}

/// Function body: params + optional return type + block + end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionBody {
    pub span: Span,
    /// Luau: `<T, U...>` generic list before the parameter parens.
    pub generics: Option<Box<GenericTypeList>>,
    pub params_parens: ContainedSpan,
    pub params: Punctuated<Parameter>,
    pub vararg: Option<VarArgParam>,
    /// Luau: `: T` return annotation after `)` - (colon span, type).
    pub return_type: Option<(Span, Type)>,
    pub block: Block,
    pub end_token: Span,
}

/// A typed name: function parameter or generic-for loop binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub span: Span,
    pub name: Token,
    /// Luau: `: T` - (colon span, type).
    pub type_annotation: Option<(Span, Type)>,
}

/// Vararg parameter (`...` or `...name` in Lua 5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarArgParam {
    pub span: Span,
    pub dots: Span,
    pub name: Option<Token>,
    /// Luau: `: T` - (colon span, type). The type may be a pack (`...number`).
    pub type_annotation: Option<(Span, Type)>,
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
        brackets: ContainedSpan,
        key: Expression,
        equal: Span,
        value: Expression,
    },
    Named {
        span: Span,
        name: Token,
        equal: Span,
        value: Expression,
    },
    Positional {
        span: Span,
        value: Expression,
    },
}
