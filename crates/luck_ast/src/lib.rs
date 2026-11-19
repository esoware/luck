//! # `luck_ast`
//!
//! The syntax tree of Lua 5.1 to 5.5 and Luau, and what walks, measures,
//! queries, and builds it. Every dialect's source reads into one shape. A
//! tree's root is a [`Chunk`], which records the [`LuaVersion`] it's in and
//! holds its [`Block`], a list of [`Statement`]s, and every value position
//! is an [`Expression`].
//!
//! [`LuaVersion`]: luck_token::LuaVersion
//!
//! A tree lives in a `luck_allocator` arena: every node is in a `Box` or a
//! `List` there, and none needs dropping, so a tree is freed with its arena
//! all at once. [`Expression`], [`Statement`], and [`Type`] are two words
//! each. A [`Name`] holds its text, and a literal, a [`NumberLiteral`],
//! [`IntegerLiteral`], or [`StringLiteral`], its value, which the parser
//! decodes once, and its spelling as written, which a tree built in code
//! leaves out. A parsed tree's leaves borrow the source, and a built tree's
//! live in the arena, so a printer never needs the source. A name also holds
//! a [`NameIndex`], a plain field that the parser numbers and a pass that
//! holds the tree mutably may renumber. A tree is `Send` and `Sync`, and may
//! join nodes from any number of arenas that outlive it.
//!
//! `luck_ast_tools` generates the tree's support code from its types. Two
//! traversals share one walk: [`Visit`] borrows the
//! tree, and [`VisitMut`] rewrites it in place. Each has a `visit_*` hook for
//! every type, to override, and a `walk_*` method that visits what a node
//! holds in source order, to call from it and recurse. [`Visit::enter_node`]
//! is handed every node as an [`AstKind`]. [`GetSpan`] gives any node's span,
//! [`ContentEq`] compares two trees leaving their spans and spellings out,
//! and every node type has a `new` constructor, a `boxed` one where a field
//! boxes it, and an enum a `new_*` one for each variant that holds a struct.
//! [`Synth`] builds on those, adding the checks a tool that emits a tree
//! needs.
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
//! let expr = synth.number_f64(0.5);
//! assert!(matches!(expr, Expression::Number(literal) if literal.value.to_f64() == 0.5));
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
mod content_eq;
mod counts;
mod depth;
mod expr;
#[cfg(test)]
mod fixtures;
mod function;
mod generated;
mod leaf;
mod one_or_more;
mod query;
mod span;
mod stmt;
mod synth;
mod take;
mod types;

pub use chain::{Call, ChainHead, DottedPath};
pub use content_eq::ContentEq;
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
pub use generated::{AstKind, AstType, Visit, VisitMut};
pub use leaf::{IntegerLiteral, Name, NameIndex, NumberLiteral, StringLiteral};
pub use one_or_more::OneOrMore;
pub use query::{
    expr_starts_with_brace, is_multi_value, is_prefix_expr, stmt_starts_with_paren,
    unwrap_parentheses,
};
pub use span::GetSpan;
pub use stmt::{
    Assignment, Attribute, AttributedName, Block, Chunk, CompoundAssignment, DoBlock, ElseClause,
    ElseIfClause, FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction,
    GlobalStar, GotoStatement, IfStatement, LabelStatement, LocalAssignment, LocalAssignmentKind,
    LocalFunction, LocalFunctionKind, NumericFor, RepeatLoop, ReturnStatement, Statement,
    TypeDeclaration, TypeDeclarationValue, WhileLoop,
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
