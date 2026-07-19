use luck_token::{Span, Token};

use crate::shared::{ContainedSpan, Field, FunctionBody, Punctuated};
use crate::types::Type;

/// A Lua expression node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expression {
    Nil(Token),
    False(Token),
    True(Token),
    Number(Token),
    StringLiteral(Token),
    VarArg(Token),
    FunctionDef(Box<FunctionDef>),
    Var(Box<Var>),
    FunctionCall(Box<FunctionCall>),
    Parenthesized(Box<ParenExpression>),
    TableConstructor(Box<TableConstructor>),
    BinaryOp(Box<BinaryOp>),
    UnaryOp(Box<UnaryOp>),
    IfExpression(Box<IfExpression>),
    InterpolatedString(Box<InterpolatedString>),
    TypeCast(Box<TypeCast>),
    Error(Span),
}

/// A variable reference: simple name, index expression, or field access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    Name(Token),
    Index(Box<IndexExpression>),
    FieldAccess(Box<FieldAccess>),
}

/// A function call: `callee(args)` or `callee:method(args)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCall {
    pub span: Span,
    pub callee: Expression,
    pub args: FunctionArgs,
    pub method: Option<(Token, Token)>,
}

/// Function call arguments: parenthesized list, table literal, or string literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionArgs {
    Parenthesized {
        parens: ContainedSpan,
        args: Punctuated<Expression>,
    },
    TableConstructor(Box<TableConstructor>),
    StringLiteral(Token),
}

/// Binary operator expression: `left op right`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryOp {
    pub span: Span,
    pub left: Expression,
    pub op: Token,
    pub right: Expression,
}

/// Unary operator expression: `op operand`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnaryOp {
    pub span: Span,
    pub op: Token,
    pub operand: Expression,
}

/// Parenthesized expression: `(expr)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParenExpression {
    pub span: Span,
    pub parens: ContainedSpan,
    pub expr: Expression,
}

/// Table constructor: `{ fields }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableConstructor {
    pub span: Span,
    pub braces: ContainedSpan,
    pub fields: Vec<(Field, Option<Token>)>,
}

/// Index expression: `prefix[index]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexExpression {
    pub span: Span,
    pub prefix: Expression,
    pub brackets: ContainedSpan,
    pub index: Expression,
}

/// Field access: `prefix.name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldAccess {
    pub span: Span,
    pub prefix: Expression,
    pub dot: Token,
    pub name: Token,
}

/// Anonymous function definition: `function(params) body end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDef {
    pub span: Span,
    pub function_token: Token,
    pub body: FunctionBody,
}

/// Luau if-expression: `if cond then expr {elseif cond then expr} else expr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfExpression {
    pub span: Span,
    pub if_token: Token,
    pub condition: Expression,
    pub then_token: Token,
    pub then_expr: Expression,
    pub elseif_clauses: Vec<ElseIfExprClause>,
    pub else_token: Token,
    pub else_expr: Expression,
}

/// An `elseif` clause within a Luau if-expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseIfExprClause {
    pub span: Span,
    pub elseif_token: Token,
    pub condition: Expression,
    pub then_token: Token,
    pub expr: Expression,
}

/// Luau interpolated string: `` `text{expr}text` ``.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpolatedString {
    pub span: Span,
    pub segments: Vec<InterpSegment>,
}

/// A segment of an interpolated string: literal text followed by an optional expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpSegment {
    pub literal: Token,
    pub expr: Option<Expression>,
}

/// Luau type cast (`expr :: Type`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeCast {
    pub span: Span,
    pub expr: Expression,
    pub double_colon: Token,
    pub type_annotation: Type,
}
