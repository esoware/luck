//! # `luck_ast`
//!
//! AST node types and traversal infrastructure for Lua/Luau. A [`Block`] holds
//! [`Statement`]s and an optional last statement; every value position is an
//! [`Expression`].
//!
//! Two traversals share one shape: [`Visitor`] borrows the tree, and
//! [`Transform`] takes each node by value and returns its replacement, as the
//! minifier's passes do. Override the `visit_*` or `transform_*` method you
//! care about and call `self.walk_*` to recurse.
//!
//! A run of suffixes (`a.b[c]:d()`) is one [`SuffixedExpression`], and a
//! run of binary operators at one precedence level (`a + b - c`) is one
//! [`BinaryOp`], so however long a run gets, it nests one level.
//!
//! Every traversal recurses on the plain stack, once per level of nesting.
//! `luck_parser` builds no tree deeper than [`MAX_NESTING_DEPTH`]. Nothing
//! checks a tree built with [`Synth`] as it grows, so measure one with
//! [`block_nesting_depth`] or [`expr_nesting_depth`], which are safe at any
//! depth.
//!
//! # Usage
//!
//! ```
//! use luck_ast::{Expression, Synth};
//!
//! let synth = Synth::new();
//! let expr = synth.number("42");
//! assert!(matches!(expr, Expression::Number(_)));
//! ```

#![cfg_attr(
    test,
    expect(
        clippy::missing_panics_doc,
        reason = "a test panics to fail, so a Panics section says nothing"
    )
)]

extern crate alloc;

mod chain;
mod depth;
mod expr;
mod function;
#[cfg(test)]
mod layout;
mod one_or_more;
mod punctuated;
mod query;
mod span;
mod stmt;
mod synth;
mod transform;
mod types;
mod visitor;

pub use chain::{Call, ChainHead};
pub use depth::{MAX_NESTING_DEPTH, block_nesting_depth, expr_nesting_depth};
pub use expr::{
    BinaryLink, BinaryOp, CallSuffix, ElseIfExprClause, Expression, Field, FunctionArgs,
    FunctionDef, IfExpression, InterpolatedString, InterpolatedStringSegment, Literal,
    ParenthesizedExpression, Suffix, SuffixedExpression, TableConstructor, TypeCast, UnaryOp, Var,
};
pub use function::{FunctionAttribute, FunctionBody, Parameter, VarArgParam};
pub use one_or_more::OneOrMore;
pub use punctuated::Punctuated;
pub use query::{
    DottedPath, expr_starts_with_brace, is_multi_value, is_prefix_expr, stmt_starts_with_paren,
    unwrap_parentheses,
};
pub use stmt::{
    Assignment, Attribute, AttributedName, Block, CompoundAssignment, DoBlock, ElseClause,
    ElseIfClause, FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction,
    GlobalStar, GotoStatement, IfStatement, LabelStatement, LastStatement, LocalAssignment,
    LocalFunction, NumericFor, RepeatLoop, ReturnStatement, Statement, TypeDeclaration,
    TypeDeclarationValue, WhileLoop,
};
pub use synth::{
    FieldSpec, FunctionSignature, InterpolatedStringPart, Synth, SyntheticComment, TypeFieldAccess,
    TypeFieldSpec, is_valid_identifier, is_valid_identifier_in, sanitize_identifier,
    sanitize_identifier_in,
};
pub use transform::Transform;
pub use types::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, TableType, Type, TypeArgs,
    TypeField, TypePack, TypeofType, UnionType, VariadicType,
};
pub use visitor::Visitor;
