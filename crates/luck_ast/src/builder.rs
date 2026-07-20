use luck_token::Span;

use crate::expr::*;
use crate::shared::*;
use crate::stmt::*;
use crate::types::*;

impl<T> Punctuated<T> {
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.items.iter_mut()
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.items.get(index)
    }

    pub fn first(&self) -> Option<&T> {
        self.items.first()
    }

    pub fn last_item(&self) -> Option<&T> {
        self.items.last()
    }

    pub fn into_items(self) -> Vec<T> {
        self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn from_item(item: T) -> Self {
        Self {
            items: vec![item],
            has_trailing_separator: false,
        }
    }

    pub fn from_items(items: Vec<T>) -> Self {
        Self {
            items,
            has_trailing_separator: false,
        }
    }

    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            has_trailing_separator: false,
        }
    }

    pub fn push(&mut self, item: T) {
        self.items.push(item);
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
            Expression::Number(t) | Expression::StringLiteral(t) => t.span,
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
