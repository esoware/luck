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

mod expr;
mod function;
#[cfg(test)]
mod layout;
mod node;
mod punctuated;
mod query;
mod span;
mod stmt;
mod synth;
mod transform;
mod types;
mod visitor;

pub use expr::{
    BinaryOp, ElseIfExprClause, Expression, Field, FieldAccess, FunctionArgs, FunctionCall,
    FunctionDef, IfExpression, IndexExpression, InterpolatedString, InterpolatedStringSegment,
    Literal, ParenthesizedExpression, TableConstructor, TypeCast, TypeInstantiation, UnaryOp, Var,
};
pub use function::{FunctionAttribute, FunctionBody, Parameter, VarArgParam};
pub use node::{NodeKind, NodeType, NodeTypeSet};
pub use punctuated::Punctuated;
pub use query::{
    expr_starts_with_brace, is_multi_value, is_prefix_expression, stmt_starts_with_paren,
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
