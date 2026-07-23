//! Luau type grammar nodes.
//!
//! Types appear in the annotation positions (parameters, varargs, return
//! types, local/loop bindings, casts) and in `type` declarations. Outside
//! Luau these nodes never occur; the parser gates on `LuaVersion::is_luau`.

use luck_token::{Span, Token};

use crate::expr::Expression;
use crate::shared::Punctuated;

/// A Luau type node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Named(Box<NamedType>),
    Typeof(Box<TypeofType>),
    Table(Box<TableType>),
    Function(Box<FunctionType>),
    Optional(Box<OptionalType>),
    Union(Box<UnionType>),
    Intersection(Box<IntersectionType>),
    /// Luau: complement of a type (`~T`).
    Negation(Box<NegationType>),
    Parenthesized(Box<ParenType>),
    /// `(T, U)` / `()` - only valid where a type pack is expected
    /// (return positions, generic argument lists).
    Pack(Box<TypePack>),
    /// Literal singleton type: string literal, `true`, `false`, or `nil`.
    /// Number tokens are also accepted permissively so historically-parsed
    /// sources keep round-tripping, even though Luau proper rejects them.
    Singleton(Token),
    /// `...T` variadic pack element.
    Variadic(Box<VariadicType>),
    /// `T...` generic pack reference.
    GenericPack(Box<GenericPackType>),
    Error(Span),
}

/// Type reference: `Name`, `module.Name`, `Name<args>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedType {
    pub span: Span,
    /// `module.` qualification.
    pub prefix: Option<Token>,
    pub name: Token,
    pub generics: Option<TypeArgs>,
}

/// Generic argument list at a type use site: `<T, U...>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeArgs {
    pub span: Span,
    pub args: Punctuated<Type>,
}

/// `typeof(expr)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeofType {
    pub span: Span,
    pub expr: Expression,
}

/// Table type: `{ name: T }`, `{ [K]: V }`, `{ T }` (array shorthand).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableType {
    pub span: Span,
    pub fields: Punctuated<TypeField>,
}

/// One entry in a table type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeField {
    Named {
        span: Span,
        /// Luau `read`/`write` access modifier.
        access: Option<Token>,
        name: Token,
        value: Type,
    },
    Indexer {
        span: Span,
        /// Luau `read`/`write` access modifier.
        access: Option<Token>,
        key: Type,
        value: Type,
    },
    /// Array shorthand `{ T }` - a bare element type.
    Array { span: Span, value: Type },
}

/// Function type: `<T>(params) -> return_type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionType {
    pub span: Span,
    pub generics: Option<GenericTypeList>,
    pub params: Punctuated<FunctionTypeParam>,
    pub return_type: Type,
}

/// One parameter in a function type, optionally named: `x: number`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionTypeParam {
    pub span: Span,
    pub name: Option<Token>,
    pub type_value: Type,
}

/// Postfix optional: `T?`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionalType {
    pub span: Span,
    pub type_value: Type,
}

/// N-ary union `A | B | C`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionType {
    pub span: Span,
    /// Leading `|` (allowed in multiline definitions).
    pub has_leading_pipe: bool,
    pub types: Punctuated<Type>,
}

/// N-ary intersection `A & B & C`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntersectionType {
    pub span: Span,
    /// Leading `&` (allowed in multiline definitions).
    pub has_leading_ampersand: bool,
    pub types: Punctuated<Type>,
}

/// Luau negation type: `~T`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegationType {
    pub span: Span,
    pub type_value: Type,
}

/// Parenthesized type: `(T)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParenType {
    pub span: Span,
    pub type_value: Type,
}

/// Explicit type pack: `(T, U)` or `()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypePack {
    pub span: Span,
    pub types: Punctuated<Type>,
}

/// Variadic pack element: `...T`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariadicType {
    pub span: Span,
    pub type_value: Type,
}

/// Generic pack reference: `T...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericPackType {
    pub span: Span,
    pub name: Token,
}

/// Generic parameter list at a declaration site:
/// `<T, U = string, V... = ...number>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericTypeList {
    pub span: Span,
    pub params: Punctuated<GenericTypeParam>,
}

/// One declared generic parameter, optionally a pack and/or defaulted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericTypeParam {
    pub span: Span,
    pub name: Token,
    /// `...` marking a pack parameter (`T...`).
    pub is_pack: bool,
    /// `= T` default. Only legal in `type` declarations.
    pub default: Option<Type>,
}
