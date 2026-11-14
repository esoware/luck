//! Types appear in the annotation positions (parameters, varargs, return
//! types, local/loop bindings, casts) and in `type` declarations.
//!
//! Outside Luau these nodes never occur; the parser gates on `LuaVersion::has_type_syntax`.

use luck_token::{CompactString, Span, Token};

use crate::expr::Expression;
use crate::punctuated::Punctuated;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Name {
        name: CompactString,
        span: Span,
    },
    Named(Box<NamedType>),
    Typeof(Box<TypeofType>),
    Table(Box<TableType>),
    Function(Box<FunctionType>),
    Optional(Box<OptionalType>),
    Union(Box<UnionType>),
    Intersection(Box<IntersectionType>),
    Parenthesized(Box<ParenthesizedType>),
    /// `(T, U)` or `()`, valid only where a type pack is expected
    /// (return positions, generic argument lists).
    Pack(Box<TypePack>),
    /// Literal singleton type: a string literal, `true`, `false`, or `nil`.
    /// The parser also accepts numbers, which Luau rejects, so sources
    /// carrying them keep round-tripping.
    Singleton(Token),
    Variadic(Box<VariadicType>),
    GenericPack(Box<GenericPackType>),
    Error(Span),
}

/// A bare `Name` is a [`Type::Name`] instead, which keeps the most common
/// type allocation-free. The parser and `Synth` only build this node with a
/// prefix or generics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedType {
    pub span: Span,
    pub prefix: Option<Token>,
    pub name: Token,
    pub generics: Option<TypeArgs>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeArgs {
    pub span: Span,
    pub args: Punctuated<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeofType {
    pub span: Span,
    pub expr: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableType {
    pub span: Span,
    pub fields: Punctuated<TypeField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeField {
    Named { span: Span, access: Option<Token>, name: Token, value: Type },
    Indexer { span: Span, access: Option<Token>, key: Type, value: Type },
    Array { span: Span, access: Option<Token>, value: Type },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionType {
    pub span: Span,
    pub generics: Option<GenericTypeList>,
    pub params: Punctuated<FunctionTypeParam>,
    pub return_type: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionTypeParam {
    pub span: Span,
    pub name: Option<Token>,
    pub type_value: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionalType {
    pub span: Span,
    pub type_value: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionType {
    pub span: Span,
    pub has_leading_pipe: bool,
    pub types: Punctuated<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntersectionType {
    pub span: Span,
    pub has_leading_ampersand: bool,
    pub types: Punctuated<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParenthesizedType {
    pub span: Span,
    pub type_value: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypePack {
    pub span: Span,
    pub types: Punctuated<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariadicType {
    pub span: Span,
    pub type_value: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericPackType {
    pub span: Span,
    pub name: Token,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericTypeList {
    pub span: Span,
    pub params: Punctuated<GenericTypeParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericTypeParam {
    pub span: Span,
    pub name: Token,
    pub is_pack: bool,
    /// Only legal in type alias declarations.
    pub default: Option<Type>,
}
