//! Types appear in the annotation positions (parameters, varargs, return
//! types, local/loop bindings, casts) and in `type` declarations.
//!
//! Outside Luau these nodes never occur; the parser gates on `LuaVersion::has_type_syntax`.

use luck_allocator::{Box, List};
use luck_token::Span;

use crate::expr::Expression;
use crate::leaf::{Literal, Name};

/// Two words, like [`Expression`].
#[derive(Debug, PartialEq, Eq)]
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
    StringLiteral(Box<'ast, Literal<'ast>>),
    /// A numeral, which Luau rejects as a type. The parser accepts one so a
    /// source carrying it keeps round-tripping.
    Number(Box<'ast, Literal<'ast>>),
    Variadic(Box<'ast, VariadicType<'ast>>),
    GenericPack(Box<'ast, GenericPackType<'ast>>),
    Error(Span),
}

/// A bare `Name` is a [`Type::Name`] instead. The parser and `Synth` only
/// build this node with a prefix or generics.
#[derive(Debug, PartialEq, Eq)]
pub struct NamedType<'ast> {
    pub span: Span,
    /// The module of `module.Name`.
    pub prefix: Option<Name<'ast>>,
    pub name: Name<'ast>,
    pub generics: Option<TypeArgs<'ast>>,
}

/// `<T, U>`, where `span` covers the angle brackets.
#[derive(Debug, PartialEq, Eq)]
pub struct TypeArgs<'ast> {
    pub span: Span,
    pub args: List<'ast, Type<'ast>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TypeofType<'ast> {
    pub span: Span,
    pub expr: Expression<'ast>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TableType<'ast> {
    pub span: Span,
    pub fields: List<'ast, TypeField<'ast>>,
    /// Whether a `,` or `;` follows the last field. The printers write `,`.
    pub has_trailing_separator: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TypeField<'ast> {
    Named { span: Span, access: Option<TypeFieldAccess>, name: Name<'ast>, value: Type<'ast> },
    Indexer { span: Span, access: Option<TypeFieldAccess>, key: Type<'ast>, value: Type<'ast> },
    Array { span: Span, access: Option<TypeFieldAccess>, value: Type<'ast> },
}

/// The `read` or `write` before a table type's field, which starts its span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, PartialEq, Eq)]
pub struct FunctionType<'ast> {
    pub span: Span,
    pub generics: Option<Box<'ast, GenericTypeList<'ast>>>,
    pub params: List<'ast, FunctionTypeParam<'ast>>,
    pub return_type: Type<'ast>,
}

/// `name: T` or `T`, spanning from whichever comes first, so it holds no span
/// of its own.
#[derive(Debug, PartialEq, Eq)]
pub struct FunctionTypeParam<'ast> {
    pub name: Option<Name<'ast>>,
    pub ty: Type<'ast>,
}

/// One `?`, so `T??` is an optional of an optional.
#[derive(Debug, PartialEq, Eq)]
pub struct OptionalType<'ast> {
    pub span: Span,
    pub ty: Type<'ast>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct UnionType<'ast> {
    pub span: Span,
    pub has_leading_pipe: bool,
    pub types: List<'ast, Type<'ast>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct IntersectionType<'ast> {
    pub span: Span,
    pub has_leading_ampersand: bool,
    pub types: List<'ast, Type<'ast>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParenthesizedType<'ast> {
    pub span: Span,
    pub ty: Type<'ast>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TypePack<'ast> {
    pub span: Span,
    pub types: List<'ast, Type<'ast>>,
}

/// `...T`.
#[derive(Debug, PartialEq, Eq)]
pub struct VariadicType<'ast> {
    pub span: Span,
    pub ty: Type<'ast>,
}

/// `T...`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenericPackType<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GenericTypeList<'ast> {
    pub span: Span,
    pub params: List<'ast, GenericTypeParam<'ast>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GenericTypeParam<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
    pub is_pack: bool,
    /// Only legal in type alias declarations.
    pub default: Option<Type<'ast>>,
}
