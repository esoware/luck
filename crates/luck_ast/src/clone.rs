//! A derived clone of a recursive enum inlines every variant's clone into
//! one frame, which a deep tree pays at every level. These clone each boxed
//! variant in a frame of its own instead.

use crate::expr::Expression;
use crate::stmt::Statement;

#[inline(never)]
#[expect(
    clippy::borrowed_box,
    reason = "`Box::clone` writes the node straight into its allocation"
)]
fn clone_box<T: Clone>(node: &Box<T>) -> Box<T> {
    Box::clone(node)
}

impl Clone for Expression {
    fn clone(&self) -> Self {
        match self {
            Self::Nil(span) => Self::Nil(*span),
            Self::False(span) => Self::False(*span),
            Self::True(span) => Self::True(*span),
            Self::Number(literal) => Self::Number(literal.clone()),
            Self::Integer(literal) => Self::Integer(literal.clone()),
            Self::StringLiteral(literal) => Self::StringLiteral(literal.clone()),
            Self::VarArg(span) => Self::VarArg(*span),
            Self::FunctionDef(node) => Self::FunctionDef(clone_box(node)),
            Self::Name(token) => Self::Name(token.clone()),
            Self::Suffixed(node) => Self::Suffixed(clone_box(node)),
            Self::Parenthesized(node) => Self::Parenthesized(clone_box(node)),
            Self::TableConstructor(node) => Self::TableConstructor(clone_box(node)),
            Self::BinaryOp(node) => Self::BinaryOp(clone_box(node)),
            Self::UnaryOp(node) => Self::UnaryOp(clone_box(node)),
            Self::IfExpression(node) => Self::IfExpression(clone_box(node)),
            Self::InterpolatedString(node) => Self::InterpolatedString(clone_box(node)),
            Self::TypeCast(node) => Self::TypeCast(clone_box(node)),
            Self::Error(span) => Self::Error(*span),
        }
    }
}

impl Clone for Statement {
    fn clone(&self) -> Self {
        match self {
            Self::Assignment(node) => Self::Assignment(clone_box(node)),
            Self::FunctionCall(node) => Self::FunctionCall(clone_box(node)),
            Self::DoBlock(node) => Self::DoBlock(clone_box(node)),
            Self::WhileLoop(node) => Self::WhileLoop(clone_box(node)),
            Self::RepeatLoop(node) => Self::RepeatLoop(clone_box(node)),
            Self::IfStatement(node) => Self::IfStatement(clone_box(node)),
            Self::NumericFor(node) => Self::NumericFor(clone_box(node)),
            Self::GenericFor(node) => Self::GenericFor(clone_box(node)),
            Self::FunctionDecl(node) => Self::FunctionDecl(clone_box(node)),
            Self::LocalFunction(node) => Self::LocalFunction(clone_box(node)),
            Self::LocalAssignment(node) => Self::LocalAssignment(clone_box(node)),
            Self::EmptyStatement(span) => Self::EmptyStatement(*span),
            Self::Goto(node) => Self::Goto(clone_box(node)),
            Self::Label(node) => Self::Label(clone_box(node)),
            Self::GlobalDeclaration(node) => Self::GlobalDeclaration(clone_box(node)),
            Self::GlobalFunction(node) => Self::GlobalFunction(clone_box(node)),
            Self::GlobalStar(node) => Self::GlobalStar(clone_box(node)),
            Self::Break(span) => Self::Break(*span),
            Self::CompoundAssignment(node) => Self::CompoundAssignment(clone_box(node)),
            Self::TypeDeclaration(node) => Self::TypeDeclaration(clone_box(node)),
            Self::Error(span) => Self::Error(*span),
        }
    }
}
