use luck_token::{BinOp, CompactString, Span, Token, UnOp};

use crate::function::{FunctionAttribute, FunctionBody};
use crate::punctuated::Punctuated;
use crate::types::{Type, TypeArgs};

/// Fixed-spelling leaves (`nil`, `true`, `false`, `...`) carry only their
/// span; number and string leaves carry a [`Literal`] with their text.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::enum_variant_names,
    reason = "each variant is named for its payload, and the syntax tree view prints these names"
)]
pub enum Expression {
    Nil(Span),
    False(Span),
    True(Span),
    Number(Literal),
    /// Luau: exact 64-bit integer literal with an `i` suffix.
    Integer(Literal),
    StringLiteral(Literal),
    VarArg(Span),
    FunctionDef(Box<FunctionDef>),
    Var(Var),
    FunctionCall(Box<FunctionCall>),
    Parenthesized(Box<ParenthesizedExpression>),
    TableConstructor(Box<TableConstructor>),
    BinaryOp(Box<BinaryOp>),
    UnaryOp(Box<UnaryOp>),
    IfExpression(Box<IfExpression>),
    InterpolatedString(Box<InterpolatedString>),
    TypeCast(Box<TypeCast>),
    /// Luau: explicit type-parameter instantiation (`f<<T>>`).
    TypeInstantiation(Box<TypeInstantiation>),
    Error(Span),
}

/// A literal leaf: its exact spelling plus its span. The owning variant
/// implies the token kind, so none is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Literal {
    pub text: CompactString,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    Name(Token),
    Index(Box<IndexExpression>),
    FieldAccess(Box<FieldAccess>),
}

#[derive(Eq)]
pub struct FunctionCall {
    pub span: Span,
    pub callee: Expression,
    pub args: FunctionArgs,
    pub method: Option<Token>,
    /// Luau: explicit type arguments between a method name and its call
    /// arguments (`receiver:method<<T>>()`). Non-method instantiation is an
    /// [`Expression::TypeInstantiation`] instead.
    pub explicit_type_args: Option<Box<TypeArgs>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionArgs {
    Parenthesized {
        /// Covers `(`..`)`.
        span: Span,
        args: Punctuated<Expression>,
    },
    TableConstructor(Box<TableConstructor>),
    StringLiteral(Literal),
}

#[derive(Eq)]
pub struct BinaryOp {
    pub span: Span,
    pub left: Expression,
    pub op: BinOp,
    pub right: Expression,
}

#[derive(Eq)]
pub struct UnaryOp {
    pub span: Span,
    pub op: UnOp,
    pub operand: Expression,
}

#[derive(Eq)]
pub struct ParenthesizedExpression {
    pub span: Span,
    pub expr: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableConstructor {
    pub span: Span,
    pub fields: Punctuated<Field>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Bracketed { span: Span, key: Expression, value: Expression },
    Named { span: Span, name: Token, value: Expression },
    Positional { span: Span, value: Expression },
}

#[derive(Eq)]
pub struct IndexExpression {
    pub span: Span,
    pub prefix: Expression,
    pub index: Expression,
}

#[derive(Eq)]
pub struct FieldAccess {
    pub span: Span,
    pub prefix: Expression,
    pub name: Token,
}

/// Anonymous function definition: `function(params) body end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDef {
    pub span: Span,
    /// Luau: `@attr` list preceding `function`. Empty outside Luau.
    pub attributes: Vec<FunctionAttribute>,
    pub body: FunctionBody,
}

/// Luau if-expression: `if cond then expr {elseif cond then expr} else expr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfExpression {
    pub span: Span,
    pub condition: Expression,
    pub then_expr: Expression,
    pub elseif_clauses: Vec<ElseIfExprClause>,
    pub else_expr: Expression,
}

/// An `elseif` clause within a Luau if-expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseIfExprClause {
    pub span: Span,
    pub condition: Expression,
    pub expr: Expression,
}

/// Luau interpolated string: `` `text{expr}text` ``.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpolatedString {
    pub span: Span,
    pub segments: Vec<InterpolatedStringSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpolatedStringSegment {
    pub literal: Token,
    pub expr: Option<Expression>,
}

/// Luau type cast (`expr :: Type`).
#[derive(Eq)]
pub struct TypeCast {
    pub span: Span,
    pub expr: Expression,
    pub type_annotation: Type,
}

/// Luau explicit type-parameter instantiation (`expr<<T, U>>`).
#[derive(Eq)]
pub struct TypeInstantiation {
    pub span: Span,
    pub expr: Expression,
    pub type_args: TypeArgs,
}
