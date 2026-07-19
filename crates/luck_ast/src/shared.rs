use luck_token::{Span, Token};

use crate::expr::Expression;
use crate::stmt::{LastStatement, Statement};
use crate::types::{GenericTypeList, Type};

/// A sequence of items separated by tokens (typically commas).
///
/// Each item carries its FOLLOWING separator; the final item's is
/// `None` (a `Some` on the final item preserves a dangling separator
/// from parse recovery). Same shape as table-constructor fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Punctuated<T> {
    pub items: Vec<(T, Option<Token>)>,
}

/// Paired delimiters (parens, brackets, braces).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainedSpan {
    pub open: Token,
    pub close: Token,
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
    /// Luau: `: T` return annotation after `)` - (colon, type).
    pub return_type: Option<(Token, Type)>,
    pub block: Block,
    pub end_token: Token,
}

/// A typed name: function parameter or generic-for loop binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub span: Span,
    pub name: Token,
    /// Luau: `: T` - (colon, type).
    pub type_annotation: Option<(Token, Type)>,
}

/// Vararg parameter (`...` or `...name` in Lua 5.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarArgParam {
    pub span: Span,
    pub dots: Token,
    pub name: Option<Token>,
    /// Luau: `: T` - (colon, type). The type may be a pack (`...number`).
    pub type_annotation: Option<(Token, Type)>,
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
        equal: Token,
        value: Expression,
    },
    Named {
        span: Span,
        name: Token,
        equal: Token,
        value: Expression,
    },
    Positional {
        span: Span,
        value: Expression,
    },
}
