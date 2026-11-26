//! The statements of a definition file, which only `luck_parser`'s
//! definition parse reads, as Luau's parser reads them with
//! `allowDeclarationSyntax`.

use luck_allocator::{Box, List};
use luck_ast_macros::Ast;
use luck_token::Span;

use crate::expr::SuffixedExpression;
use crate::function::{FunctionAttribute, VarArgParam};
use crate::leaf::Name;
use crate::stmt::TypedName;
use crate::types::{GenericTypeList, Type, TypeField};

/// `declare name: T`, a global of type `T`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct DeclaredGlobal<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
    pub ty: Type<'ast>,
}

/// A function declared without a body: the global of `declare function
/// name<T>(x: T): R`, or an extern type's method, `function name(self, x:
/// T): R`, which takes no generics.
///
/// Every parameter is annotated, but for a method's first, which is an
/// unannotated `self`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct DeclaredFunction<'ast> {
    /// From the first attribute, or from `declare` or `function`, to the end
    /// of the signature.
    pub span: Span,
    pub attributes: List<'ast, FunctionAttribute<'ast>>,
    /// The chain an attributed `declare` opens, as `declare.x` in
    /// `@checked declare.x function f()`, which Luau's parser reads as an
    /// expression and drops. The printers keep it.
    pub declare_chain: Option<Box<'ast, SuffixedExpression<'ast>>>,
    pub name: Name<'ast>,
    pub generics: Option<Box<'ast, GenericTypeList<'ast>>>,
    pub params: List<'ast, TypedName<'ast>>,
    pub vararg: Option<Box<'ast, VarArgParam<'ast>>>,
    pub return_type: Option<Box<'ast, Type<'ast>>>,
}

/// `declare extern type Name extends Supertype with ... end`, a type the
/// host defines, such as a Roblox class.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct DeclaredExternType<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
    /// The type after `extends`.
    pub supertype: Option<Name<'ast>>,
    pub members: List<'ast, ExternTypeMember<'ast>>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub enum ExternTypeMember<'ast> {
    /// A property, `[read | write] name: T` or `["name"]: T`, or the indexer,
    /// `[K]: V`, as a table type holds them. An extern type holds no array
    /// field, takes `read` or `write` only before a name, and holds one
    /// indexer at most.
    Field(TypeField<'ast>),
    /// A method, which a repeated name overloads.
    Method(Box<'ast, DeclaredFunction<'ast>>),
}
