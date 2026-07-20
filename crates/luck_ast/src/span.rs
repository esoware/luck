//! `span()` accessors for the node enums, kept together so span extraction
//! is one greppable surface rather than scattered across the node modules.

use luck_token::Span;

use crate::expr::{Expression, Var};
use crate::shared::Field;
use crate::stmt::{LastStatement, Statement};
use crate::types::{Type, TypeField};

impl Statement {
    pub fn span(&self) -> Span {
        match self {
            Statement::Assignment(s) => s.span,
            Statement::FunctionCall(s) => s.span,
            Statement::DoBlock(s) => s.span,
            Statement::WhileLoop(s) => s.span,
            Statement::RepeatLoop(s) => s.span,
            Statement::IfStatement(s) => s.span,
            Statement::NumericFor(s) => s.span,
            Statement::GenericFor(s) => s.span,
            Statement::FunctionDecl(s) => s.span,
            Statement::LocalFunction(s) => s.span,
            Statement::LocalAssignment(s) => s.span,
            Statement::Goto(s) => s.span,
            Statement::Label(s) => s.span,
            Statement::GlobalDeclaration(s) => s.span,
            Statement::GlobalFunction(s) => s.span,
            Statement::GlobalStar(s) => s.span,
            Statement::CompoundAssignment(s) => s.span,
            Statement::TypeDeclaration(s) => s.span,
            Statement::EmptyStatement(span) | Statement::Break(span) => *span,
            Statement::Error(span) => *span,
        }
    }
}

impl LastStatement {
    pub fn span(&self) -> Span {
        match self {
            LastStatement::Return(s) => s.span,
            LastStatement::Break(span) | LastStatement::Continue(span) => *span,
            LastStatement::Error(span) => *span,
        }
    }
}

impl Expression {
    pub fn span(&self) -> Span {
        match self {
            Expression::Nil(span)
            | Expression::False(span)
            | Expression::True(span)
            | Expression::VarArg(span) => *span,
            Expression::Number(literal) | Expression::StringLiteral(literal) => literal.span,
            Expression::FunctionDef(e) => e.span,
            Expression::Var(e) => e.span(),
            Expression::FunctionCall(e) => e.span,
            Expression::Parenthesized(e) => e.span,
            Expression::TableConstructor(e) => e.span,
            Expression::BinaryOp(e) => e.span,
            Expression::UnaryOp(e) => e.span,
            Expression::IfExpression(e) => e.span,
            Expression::InterpolatedString(e) => e.span,
            Expression::TypeCast(e) => e.span,
            Expression::Error(span) => *span,
        }
    }
}

impl Var {
    pub fn span(&self) -> Span {
        match self {
            Var::Name(token) => token.span,
            Var::Index(e) => e.span,
            Var::FieldAccess(e) => e.span,
        }
    }
}

impl Field {
    pub fn span(&self) -> Span {
        match self {
            Field::Bracketed { span, .. } => *span,
            Field::Named { span, .. } => *span,
            Field::Positional { span, .. } => *span,
        }
    }
}

impl Type {
    pub fn span(&self) -> Span {
        match self {
            Type::Named(t) => t.span,
            Type::Typeof(t) => t.span,
            Type::Table(t) => t.span,
            Type::Function(t) => t.span,
            Type::Optional(t) => t.span,
            Type::Union(t) => t.span,
            Type::Intersection(t) => t.span,
            Type::Parenthesized(t) => t.span,
            Type::Pack(t) => t.span,
            Type::Singleton(t) => t.span,
            Type::Variadic(t) => t.span,
            Type::GenericPack(t) => t.span,
            Type::Error(span) => *span,
        }
    }
}

impl TypeField {
    pub fn span(&self) -> Span {
        match self {
            TypeField::Named { span, .. } => *span,
            TypeField::Indexer { span, .. } => *span,
            TypeField::Array { span, .. } => *span,
        }
    }
}
