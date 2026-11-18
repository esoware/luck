//! `span()` accessors, and `take()` where a node has a placeholder that costs
//! no allocation, kept in one place instead of beside each node type.

use core::mem;

use luck_allocator::List;
use luck_token::Span;

use crate::expr::{Expression, Field, FunctionArgs, Suffix, Var};
use crate::function::Parameter;
use crate::stmt::{Block, LastStatement, Statement};
use crate::types::{FunctionTypeParam, Type, TypeField};

impl Block<'_> {
    /// Moves the value out of a `&mut` and leaves an empty block behind, as
    /// [`Expression::take`] does.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self { span: Span::new(0, 0), stmts: List::new(), last_stmt: None })
    }
}

impl Statement<'_> {
    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind, as [`Expression::take`] does.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::new(0, 0)))
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

impl LastStatement<'_> {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Return(stmt) => stmt.span,
            Self::Break(span) | Self::Continue(span) | Self::Error(span) => *span,
        }
    }
}

impl Expression<'_> {
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
            Self::Name(name) => name.span,
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
    /// behind, which costs no allocation. Assign the replacement back.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::new(0, 0)))
    }
}

impl Var<'_> {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Name(name) => name.span,
            Self::Suffixed(chain) => chain.span,
        }
    }
}

impl Suffix<'_> {
    /// A field's span is its name's, and a method call's starts at the
    /// method's name.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Field(name) => name.span,
            Self::Index { span, .. } | Self::TypeInstantiation { span, .. } => *span,
            Self::Call(args) => args.span(),
            Self::MethodCall(call) => call.name.span.merge(call.args.span()),
        }
    }
}

impl FunctionArgs<'_> {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Parenthesized { span, .. } | Self::Error(span) => *span,
            Self::TableConstructor(table) => table.span,
            Self::StringLiteral(literal) => literal.span,
        }
    }
}

impl Field<'_> {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Bracketed { span, .. } => *span,
            Self::Named { name, value } => name.span.merge(value.span()),
            Self::Positional(value) => value.span(),
        }
    }
}

impl Parameter<'_> {
    #[must_use]
    pub fn span(&self) -> Span {
        self.type_annotation.as_ref().map_or(self.name.span, |ty| self.name.span.merge(ty.span()))
    }
}

impl Type<'_> {
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Nil(span) | Self::True(span) | Self::False(span) | Self::Error(span) => *span,
            Self::Name(name) => name.span,
            Self::StringLiteral(literal) | Self::Number(literal) => literal.span,
            Self::Named(node) => node.span,
            Self::Typeof(node) => node.span,
            Self::Table(node) => node.span,
            Self::Function(node) => node.span,
            Self::Optional(node) => node.span,
            Self::Union(node) => node.span,
            Self::Intersection(node) => node.span,
            Self::Parenthesized(node) => node.span,
            Self::Pack(node) => node.span,
            Self::Variadic(node) => node.span,
            Self::GenericPack(node) => node.span,
        }
    }

    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind, as [`Expression::take`] does.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::new(0, 0)))
    }
}

impl TypeField<'_> {
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Named { span, .. } | Self::Indexer { span, .. } | Self::Array { span, .. } => {
                *span
            }
        }
    }
}

impl FunctionTypeParam<'_> {
    #[must_use]
    pub fn span(&self) -> Span {
        self.name.map_or_else(|| self.ty.span(), |name| name.span.merge(self.ty.span()))
    }
}
