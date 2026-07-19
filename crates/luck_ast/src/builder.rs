use luck_token::{Span, Token};

use crate::expr::*;
use crate::shared::*;
use crate::stmt::*;
use crate::types::*;

impl<T> Punctuated<T> {
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter().map(|(item, _)| item)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.items.iter_mut().map(|(item, _)| item)
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.items.get(index).map(|(item, _)| item)
    }

    pub fn first(&self) -> Option<&T> {
        self.get(0)
    }

    pub fn last_item(&self) -> Option<&T> {
        self.items.last().map(|(item, _)| item)
    }

    pub fn into_items(self) -> Vec<T> {
        self.items.into_iter().map(|(item, _)| item).collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn from_item(item: T) -> Self {
        Self {
            items: vec![(item, None)],
        }
    }

    pub fn empty() -> Self {
        Self { items: Vec::new() }
    }

    /// Append an item; `separator` is the token FOLLOWING it, if any.
    pub fn push(&mut self, item: T, separator: Option<Token>) {
        self.items.push((item, separator));
    }

    /// Build from parser-style accumulation: every item in `pairs` was
    /// followed by its separator, then an optional final item.
    pub fn from_pairs(pairs: Vec<(T, Token)>, last: Option<T>) -> Self {
        let mut items: Vec<(T, Option<Token>)> = pairs
            .into_iter()
            .map(|(item, sep)| (item, Some(sep)))
            .collect();
        if let Some(last) = last {
            items.push((last, None));
        }
        Self { items }
    }
}

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
            Statement::EmptyStatement(t) | Statement::Break(t) => t.span,
            Statement::Error(span) => *span,
        }
    }
}

impl LastStatement {
    pub fn span(&self) -> Span {
        match self {
            LastStatement::Return(s) => s.span,
            LastStatement::Break(t) | LastStatement::Continue(t) => t.span,
            LastStatement::Error(span) => *span,
        }
    }
}

impl Expression {
    pub fn span(&self) -> Span {
        match self {
            Expression::Nil(t)
            | Expression::False(t)
            | Expression::True(t)
            | Expression::Number(t)
            | Expression::StringLiteral(t)
            | Expression::VarArg(t) => t.span,
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
            Var::Name(t) => t.span,
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
