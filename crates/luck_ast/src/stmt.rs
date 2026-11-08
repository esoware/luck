use luck_token::{CompoundOp, Span, Token};

use crate::expr::{Expression, FunctionCall, Var};
use crate::function::{FunctionAttribute, FunctionBody, Parameter};
use crate::punctuated::Punctuated;
use crate::types::{GenericTypeList, Type};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub span: Span,
    pub stmts: Vec<Statement>,
    pub last_stmt: Option<Box<LastStatement>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::enum_variant_names,
    reason = "each variant is named for its payload, and the syntax tree view prints these names"
)]
pub enum Statement {
    Assignment(Box<Assignment>),
    FunctionCall(Box<FunctionCall>),
    DoBlock(Box<DoBlock>),
    WhileLoop(Box<WhileLoop>),
    RepeatLoop(Box<RepeatLoop>),
    IfStatement(Box<IfStatement>),
    NumericFor(Box<NumericFor>),
    GenericFor(Box<GenericFor>),
    FunctionDecl(Box<FunctionDecl>),
    LocalFunction(Box<LocalFunction>),
    LocalAssignment(Box<LocalAssignment>),
    EmptyStatement(Span),
    Goto(Box<GotoStatement>),
    Label(Box<LabelStatement>),
    GlobalDeclaration(Box<GlobalDeclaration>),
    GlobalFunction(Box<GlobalFunction>),
    GlobalStar(Box<GlobalStar>),
    /// Lua 5.2+: `break` as a regular statement, not only a last statement.
    Break(Span),
    CompoundAssignment(Box<CompoundAssignment>),
    TypeDeclaration(Box<TypeDeclaration>),
    Error(Span),
}

/// A block-terminating statement: `return`, `break`, or `continue` (Luau).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastStatement {
    Return(Box<ReturnStatement>),
    Break(Span),
    Continue(Span),
    Error(Span),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub span: Span,
    pub targets: Punctuated<Var>,
    pub values: Punctuated<Expression>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoBlock {
    pub span: Span,
    pub block: Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhileLoop {
    pub span: Span,
    pub condition: Expression,
    pub block: Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatLoop {
    pub span: Span,
    pub block: Block,
    pub condition: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfStatement {
    pub span: Span,
    pub condition: Expression,
    pub block: Block,
    pub elseif_clauses: Vec<ElseIfClause>,
    pub else_clause: Option<ElseClause>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseIfClause {
    pub span: Span,
    pub condition: Expression,
    pub block: Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseClause {
    pub span: Span,
    pub block: Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericFor {
    pub span: Span,
    pub name: Token,
    /// Luau: `: T` on the loop variable.
    pub type_annotation: Option<Type>,
    pub start: Expression,
    pub limit: Expression,
    pub step: Option<Expression>,
    pub block: Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericFor {
    pub span: Span,
    pub names: Punctuated<Parameter>,
    pub exprs: Punctuated<Expression>,
    pub block: Block,
}

/// Dotted function name with optional method: `a.b.c:method`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionName {
    pub span: Span,
    pub names: Vec<Token>,
    pub method: Option<Token>,
}

/// Named function declaration: `function a.b.c:m(...) ... end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub span: Span,
    /// Luau: `@attr` list preceding `function`. Empty outside Luau.
    pub attributes: Vec<FunctionAttribute>,
    pub name: FunctionName,
    pub body: FunctionBody,
}

/// Local function declaration: `local function name(...) ... end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFunction {
    pub span: Span,
    /// Luau: `@attr` list preceding `local function`. Empty outside Luau.
    pub attributes: Vec<FunctionAttribute>,
    pub name: Token,
    pub body: FunctionBody,
    /// Luau `const function NAME funcbody`. The emitter prints `const`
    /// in place of `local`.
    pub is_const: bool,
    /// Luau: `export function NAME funcbody`.
    pub is_exported: bool,
}

/// Lua 5.4+ variable attribute: `<const>` or `<close>`.
///
/// The name stays a token because the parser keeps whatever identifier it
/// finds there, after reporting any name other than `const` or `close`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub span: Span,
    pub name: Token,
}

/// One declared name with its optional attribute (`x <const>`) or Luau
/// annotation (`x: T`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributedName {
    pub name: Token,
    /// Luau: `: T` annotation. The parser never sets both this and
    /// `attribute`, since attributes are Lua 5.4+ only.
    pub type_annotation: Option<Type>,
    pub attribute: Option<Box<Attribute>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAssignment {
    pub span: Span,
    pub names: Punctuated<AttributedName>,
    pub exprs: Option<Punctuated<Expression>>,
    /// Luau `const bindinglist = explist`. The emitter prints `const` in
    /// place of `local`, and every name in the list is read-only.
    pub is_const: bool,
    /// Luau: `export local` or `export const`.
    pub is_exported: bool,
}

/// `goto name` statement (Lua 5.2+).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoStatement {
    pub span: Span,
    pub name: Token,
}

/// `::name::` label statement (Lua 5.2+).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelStatement {
    pub span: Span,
    pub name: Token,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnStatement {
    pub span: Span,
    pub exprs: Punctuated<Expression>,
}

/// Luau compound assignment, for example `x += 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundAssignment {
    pub span: Span,
    pub var: Var,
    pub op: CompoundOp,
    pub expr: Expression,
}

/// Luau type declaration: an alias (`type Name = T`) or a type function
/// (`type function Name funcbody`), as [`TypeDeclarationValue`] records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDeclaration {
    pub span: Span,
    pub is_exported: bool,
    pub name: Token,
    pub generics: Option<Box<GenericTypeList>>,
    pub type_value: TypeDeclarationValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDeclarationValue {
    Alias(Type),
    /// `type function Name funcbody`, a compile-time function evaluated
    /// during type checking. Its body is ordinary Luau.
    TypeFunction(Box<FunctionBody>),
}

/// Lua 5.5 `global` variable declaration: `global names [= exprs]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalDeclaration {
    pub span: Span,
    pub names: Punctuated<AttributedName>,
    pub exprs: Option<Punctuated<Expression>>,
}

/// Lua 5.5 `global function`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalFunction {
    pub span: Span,
    pub name: Token,
    pub body: FunctionBody,
}

/// Lua 5.5 `global *`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalStar {
    pub span: Span,
    pub attribute: Option<Attribute>,
}
