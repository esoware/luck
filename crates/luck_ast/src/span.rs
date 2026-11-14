//! `span()` accessors, and `take()` where a node has an empty placeholder,
//! kept in one place instead of beside each node type.

use core::mem;

use luck_token::Span;

use crate::expr::{Expression, Field, FunctionArgs, Suffix, Var};
use crate::punctuated::Punctuated;
use crate::stmt::{Block, LastStatement, Statement};
use crate::types::{Type, TypeField};

impl Block {
    /// Moves the value out of a `&mut` and leaves an empty block behind, as
    /// [`Expression::take`] does.
    #[must_use]
    pub fn take(&mut self) -> Self {
        mem::replace(self, Self { span: Span::default(), stmts: Vec::new(), last_stmt: None })
    }
}

impl Statement {
    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind, as [`Expression::take`] does.
    #[must_use]
    pub fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::default()))
    }

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

    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind, as [`Expression::take`] does.
    #[must_use]
    pub fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::default()))
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
            Self::Name(token) => token.span,
            Self::Suffixed(expr) => expr.span,
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
        }
    }

    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind. Assign the replacement back.
    #[must_use]
    pub fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::default()))
    }
}

impl Var {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Name(token) => token.span,
            Self::Suffixed(chain) => chain.span,
        }
    }
}

impl Suffix {
    /// A field's span is its name's, and a method call's starts at the
    /// method's name.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Field(name) => name.span,
            Self::Index { span, .. } | Self::TypeInstantiation { span, .. } => *span,
            Self::Call(call) => {
                let args_span = call.args.span();
                call.method.as_ref().map_or(args_span, |method| method.span.merge(args_span))
            }
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

    /// Moves the value out of a `&mut` and leaves an empty list behind, as
    /// [`Expression::take`] does.
    #[must_use]
    pub fn take(&mut self) -> Self {
        mem::replace(self, Self::Parenthesized { span: Span::default(), args: Punctuated::empty() })
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

    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind, as [`Expression::take`] does.
    #[must_use]
    pub fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::default()))
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
