//! Types appear in the annotation positions (parameters, varargs, return
//! types, local/loop bindings, casts) and in `type` declarations.
//!
//! Outside Luau these nodes never occur; the parser gates on `LuaVersion::has_type_syntax`.

use luck_allocator::{Box, List};
use luck_ast_macros::Ast;
use luck_token::Span;

use crate::expr::Expression;
use crate::leaf::{Name, StringLiteral};

/// Two words, like [`Expression`].
#[derive(Debug, PartialEq, Eq, Ast)]
pub enum Type<'ast> {
    /// A bare name, which keeps the most common type to one small box.
    Name(Box<'ast, Name<'ast>>),
    Named(Box<'ast, NamedType<'ast>>),
    Typeof(Box<'ast, TypeofType<'ast>>),
    Table(Box<'ast, TableType<'ast>>),
    Function(Box<'ast, FunctionType<'ast>>),
    Optional(Box<'ast, OptionalType<'ast>>),
    Union(Box<'ast, UnionType<'ast>>),
    Intersection(Box<'ast, IntersectionType<'ast>>),
    Parenthesized(Box<'ast, ParenthesizedType<'ast>>),
    /// `(T, U)` or `()`, valid only where a type pack is expected
    /// (return positions, generic argument lists).
    Pack(Box<'ast, TypePack<'ast>>),
    /// The singleton `nil`.
    Nil(Span),
    True(Span),
    False(Span),
    StringLiteral(Box<'ast, StringLiteral<'ast>>),
    Variadic(Box<'ast, VariadicType<'ast>>),
    GenericPack(Box<'ast, GenericPackType<'ast>>),
    Error(Span),
}

/// A bare `Name` is a [`Type::Name`] instead. The parser and `Synth` only
/// build this node with a prefix or generics.
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct NamedType<'ast> {
    pub span: Span,
    /// The module of `module.Name`.
    pub prefix: Option<Name<'ast>>,
    pub name: Name<'ast>,
    pub generics: Option<TypeArgs<'ast>>,
}

/// `<T, U>`, where `span` covers the angle brackets.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct TypeArgs<'ast> {
    pub span: Span,
    pub args: List<'ast, Type<'ast>>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct TypeofType<'ast> {
    pub span: Span,
    pub expr: Expression<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct TableType<'ast> {
    pub span: Span,
    pub fields: List<'ast, TypeField<'ast>>,
    /// Whether a `,` or `;` follows the last field. The printers write `,`.
    #[content_eq(skip)]
    pub has_trailing_separator: bool,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub enum TypeField<'ast> {
    Named { span: Span, access: Option<TypeFieldAccess>, name: Name<'ast>, value: Type<'ast> },
    Indexer { span: Span, access: Option<TypeFieldAccess>, key: Type<'ast>, value: Type<'ast> },
    Array { span: Span, access: Option<TypeFieldAccess>, value: Type<'ast> },
}

/// The `read` or `write` before a table type's field, which starts its span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub enum TypeFieldAccess {
    Read,
    Write,
}

impl TypeFieldAccess {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct FunctionType<'ast> {
    pub span: Span,
    pub generics: Option<Box<'ast, GenericTypeList<'ast>>>,
    pub params: List<'ast, FunctionTypeParam<'ast>>,
    pub return_type: Type<'ast>,
}

/// `name: T` or `T`, spanning from whichever comes first, so it holds no span
/// of its own.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct FunctionTypeParam<'ast> {
    pub name: Option<Name<'ast>>,
    pub ty: Type<'ast>,
}

/// Luau also accepts `T??`. The parser and `Synth` build one node for a run
/// of `?`, so `ty` is never itself an optional.
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct OptionalType<'ast> {
    pub span: Span,
    pub ty: Type<'ast>,
    /// At least one.
    pub question_count: u32,
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct UnionType<'ast> {
    pub span: Span,
    #[content_eq(skip)]
    pub has_leading_pipe: bool,
    pub types: List<'ast, Type<'ast>>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct IntersectionType<'ast> {
    pub span: Span,
    #[content_eq(skip)]
    pub has_leading_ampersand: bool,
    pub types: List<'ast, Type<'ast>>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct ParenthesizedType<'ast> {
    pub span: Span,
    pub ty: Type<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct TypePack<'ast> {
    pub span: Span,
    pub types: List<'ast, Type<'ast>>,
}

/// `...T`.
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct VariadicType<'ast> {
    pub span: Span,
    pub ty: Type<'ast>,
}

/// `T...`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub struct GenericPackType<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct GenericTypeList<'ast> {
    pub span: Span,
    pub params: List<'ast, GenericTypeParam<'ast>>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct GenericTypeParam<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
    pub is_pack: bool,
    /// Only legal in type alias declarations.
    pub default: Option<Box<'ast, Type<'ast>>>,
}
