//! `span()` accessors for the node enums, kept in one place instead of beside
//! each node type.

use luck_token::Span;

use crate::expr::{Expression, Field, FunctionArgs, Var};
use crate::stmt::{LastStatement, Statement};
use crate::types::{Type, TypeField};

impl Statement {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Assignment(stmt) => stmt.span,
            Self::FunctionCall(stmt) => stmt.span,
            Self::DoBlock(stmt) => stmt.span,
            Self::WhileLoop(stmt) => stmt.span,
            Self::RepeatLoop(stmt) => stmt.span,
            Self::IfStatement(stmt) => stmt.span,
            Self::NumericFor(stmt) => stmt.span,
            Self::GenericFor(stmt) => stmt.span,
            Self::FunctionDecl(stmt) => stmt.span,
            Self::LocalFunction(stmt) => stmt.span,
            Self::LocalAssignment(stmt) => stmt.span,
            // Lua 5.2+
            Self::Goto(stmt) => stmt.span,
            // Lua 5.2+
            Self::Label(stmt) => stmt.span,
            // Lua 5.5
            Self::GlobalDeclaration(stmt) => stmt.span,
            // Lua 5.5
            Self::GlobalFunction(stmt) => stmt.span,
            // Lua 5.5
            Self::GlobalStar(stmt) => stmt.span,
            // Luau
            Self::CompoundAssignment(stmt) => stmt.span,
            // Luau
            Self::TypeDeclaration(stmt) => stmt.span,
            Self::EmptyStatement(span) | Self::Break(span) | Self::Error(span) => *span,
        }
    }
}

impl LastStatement {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Return(stmt) => stmt.span,
            Self::Break(span) | Self::Continue(span) | Self::Error(span) => *span,
        }
    }
}

impl Expression {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Nil(span)
            | Self::False(span)
            | Self::True(span)
            | Self::VarArg(span)
            | Self::Error(span) => *span,
            Self::Number(literal) | Self::Integer(literal) | Self::StringLiteral(literal) => {
                literal.span
            }
            Self::FunctionDef(expr) => expr.span,
            Self::Var(expr) => expr.span(),
            Self::FunctionCall(expr) => expr.span,
            Self::Parenthesized(expr) => expr.span,
            Self::TableConstructor(expr) => expr.span,
            Self::BinaryOp(expr) => expr.span,
            Self::UnaryOp(expr) => expr.span,
            // Luau
            Self::IfExpression(expr) => expr.span,
            // Luau
            Self::InterpolatedString(expr) => expr.span,
            // Luau
            Self::TypeCast(expr) => expr.span,
            // Luau
            Self::TypeInstantiation(expr) => expr.span,
        }
    }
}

impl Var {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Name(token) => token.span,
            Self::Index(expr) => expr.span,
            Self::FieldAccess(expr) => expr.span,
        }
    }
}

impl FunctionArgs {
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Parenthesized { span, .. } => *span,
            Self::TableConstructor(table) => table.span,
            Self::StringLiteral(literal) => literal.span,
        }
    }
}

impl Field {
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Bracketed { span, .. }
            | Self::Named { span, .. }
            | Self::Positional { span, .. } => *span,
        }
    }
}

impl Type {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Name { span, .. } | Self::Error(span) => *span,
            Self::Named(node) => node.span,
            Self::Typeof(node) => node.span,
            Self::Table(node) => node.span,
            Self::Function(node) => node.span,
            Self::Optional(node) => node.span,
            Self::Union(node) => node.span,
            Self::Intersection(node) => node.span,
            Self::Parenthesized(node) => node.span,
            Self::Pack(node) => node.span,
            Self::Singleton(node) => node.span,
            Self::Variadic(node) => node.span,
            Self::GenericPack(node) => node.span,
        }
    }
}

impl TypeField {
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Named { span, .. } | Self::Indexer { span, .. } | Self::Array { span, .. } => {
                *span
            }
        }
    }
}
