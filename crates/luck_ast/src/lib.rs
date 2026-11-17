//! # `luck_ast`
//!
//! The syntax tree of Lua 5.1 to 5.5 and Luau, and what walks, measures,
//! queries, and builds it. A [`Block`] holds [`Statement`]s and an optional
//! last statement; every value position is an [`Expression`].
//!
//! A tree lives in a `luck_allocator` arena: every node is in a `Box` or a
//! `List` there, and none needs dropping, so a tree is freed with its arena
//! all at once. [`Expression`], [`Statement`], and [`Type`] are two words
//! each. A leaf, a [`Name`] or a [`Literal`], holds its text as written: a
//! parsed tree's borrows the source, and a built tree's lives in the arena,
//! so a printer never needs the source. A tree is `Send` and `Sync`, and
//! may join nodes from any number of arenas that outlive it.
//!
//! Two traversals share one walk: [`Visitor`] borrows the tree, and
//! [`VisitorMut`] rewrites it in place. Override the `visit_*` method you
//! care about and call `self.walk_*` to recurse.
//!
//! A run of suffixes (`a.b[c]:d()`) is one [`SuffixedExpression`], and a
//! run of binary operators at one precedence level (`a + b - c`) is one
//! [`BinaryOp`], so however long a run gets, it nests one level.
//!
//! Every traversal recurses on the plain stack, once per level of nesting.
//! `luck_parser` builds no tree deeper than [`MAX_NESTING_DEPTH`], which any
//! stage walks on a thread with [`MIN_STACK_BYTES`]. Nothing checks a tree
//! built with [`Synth`] as it grows, so measure one with
//! [`block_nesting_depth`] or [`expr_nesting_depth`], which are safe at any
//! depth.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_ast::{Expression, SpanCounter, Synth};
//!
//! let allocator = Allocator::new();
//! let spans = SpanCounter::new();
//! let synth = Synth::new(&allocator, &spans);
//! let expr = synth.number("42");
//! assert!(matches!(expr, Expression::Number(literal) if literal.text == "42"));
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
mod counts;
mod depth;
mod expr;
#[cfg(test)]
mod fixtures;
mod function;
#[cfg(test)]
mod layout;
mod leaf;
mod one_or_more;
mod query;
mod span;
mod stmt;
mod synth;
mod types;
mod visitor;
mod visitor_mut;

pub use chain::{Call, ChainHead, DottedPath};
pub use counts::TreeCounts;
pub use depth::{
    MAX_NESTING_DEPTH, MIN_STACK_BYTES, block_nesting_depth, expr_nesting_depth,
    spans_past_nesting_cap, type_nesting_depth,
};
pub use expr::{
    BinaryLink, BinaryOp, ElseIfExprClause, Expression, Field, FunctionArgs, FunctionDef,
    IfExpression, InterpolatedSegment, InterpolatedString, MethodCall, ParenthesizedExpression,
    Suffix, SuffixedExpression, TableConstructor, TypeCast, UnaryOp, Var,
};
pub use function::{FunctionAttribute, FunctionBody, Parameter, VarArgParam};
pub use leaf::{Literal, Name};
pub use one_or_more::OneOrMore;
pub use query::{
    expr_starts_with_brace, is_multi_value, is_prefix_expr, stmt_starts_with_paren,
    unwrap_parentheses,
};
pub use stmt::{
    Assignment, Attribute, AttributedName, Block, CompoundAssignment, DoBlock, ElseClause,
    ElseIfClause, FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction,
    GlobalStar, GotoStatement, IfStatement, LabelStatement, LastStatement, LocalAssignment,
    LocalAssignmentKind, LocalFunction, LocalFunctionKind, NumericFor, RepeatLoop, ReturnStatement,
    Statement, TypeDeclaration, TypeDeclarationValue, WhileLoop,
};
pub use synth::{
    FieldSpec, FunctionSignature, InterpolatedStringPart, SpanCounter, Synth, SyntheticComment,
    TypeFieldSpec, is_valid_identifier, is_valid_identifier_in, sanitize_identifier,
    sanitize_identifier_in,
};
pub use types::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, TableType, Type, TypeArgs,
    TypeField, TypeFieldAccess, TypePack, TypeofType, UnionType, VariadicType,
};
pub use visitor::Visitor;
pub use visitor_mut::VisitorMut;
