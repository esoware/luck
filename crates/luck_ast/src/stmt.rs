use luck_token::{Span, Token};

use crate::expr::{Expression, FunctionCall, Var};
use crate::shared::{Block, FunctionBody, Parameter, Punctuated};
use crate::types::{GenericTypeList, Type};

/// A Lua statement node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    Assignment(Box<Assignment>),
    FunctionCall(Box<FunctionCallStmt>),
    DoBlock(Box<DoBlock>),
    WhileLoop(Box<WhileLoop>),
    RepeatLoop(Box<RepeatLoop>),
    IfStatement(Box<IfStatement>),
    NumericFor(Box<NumericFor>),
    GenericFor(Box<GenericFor>),
    FunctionDecl(Box<FunctionDecl>),
    LocalFunction(Box<LocalFunction>),
    LocalAssignment(Box<LocalAssignment>),
    EmptyStatement(Token),
    Goto(Box<GotoStatement>),
    Label(Box<LabelStatement>),
    GlobalDeclaration(Box<GlobalDeclaration>),
    GlobalFunction(Box<GlobalFunction>),
    GlobalStar(Box<GlobalStar>),
    /// Lua 5.2+: `break` as a regular statement (not just last statement)
    Break(Token),
    CompoundAssignment(Box<CompoundAssignment>),
    TypeDeclaration(Box<TypeDeclaration>),
    Error(Span),
}

/// A block-terminating statement: `return`, `break`, or `continue` (Luau).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastStatement {
    Return(Box<ReturnStatement>),
    Break(Token),
    Continue(Token),
    Error(Span),
}

/// Multi-assignment: `targets = values`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub span: Span,
    pub targets: Punctuated<Var>,
    pub equal: Token,
    pub values: Punctuated<Expression>,
}

/// A function call used as a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCallStmt {
    pub span: Span,
    pub call: FunctionCall,
}

/// `do ... end` block statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoBlock {
    pub span: Span,
    pub do_token: Token,
    pub block: Block,
    pub end_token: Token,
}

/// `while condition do ... end` loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhileLoop {
    pub span: Span,
    pub while_token: Token,
    pub condition: Expression,
    pub do_token: Token,
    pub block: Block,
    pub end_token: Token,
}

/// `repeat ... until condition` loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatLoop {
    pub span: Span,
    pub repeat_token: Token,
    pub block: Block,
    pub until_token: Token,
    pub condition: Expression,
}

/// `if ... then ... {elseif ... then ...} [else ...] end` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfStatement {
    pub span: Span,
    pub if_token: Token,
    pub condition: Expression,
    pub then_token: Token,
    pub block: Block,
    pub elseif_clauses: Vec<ElseIfClause>,
    pub else_clause: Option<ElseClause>,
    pub end_token: Token,
}

/// An `elseif condition then ...` clause within an if statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseIfClause {
    pub span: Span,
    pub elseif_token: Token,
    pub condition: Expression,
    pub then_token: Token,
    pub block: Block,
}

/// An `else ...` clause within an if statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseClause {
    pub span: Span,
    pub else_token: Token,
    pub block: Block,
}

/// `for name = start, limit [, step] do ... end` numeric loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericFor {
    pub span: Span,
    pub for_token: Token,
    pub name: Token,
    /// Luau: `: T` on the loop variable - (colon, type).
    pub type_annotation: Option<(Token, Type)>,
    pub equal: Token,
    pub start: Expression,
    pub comma1: Token,
    pub limit: Expression,
    pub comma2_and_step: Option<(Token, Expression)>,
    pub do_token: Token,
    pub block: Block,
    pub end_token: Token,
}

/// `for names in exprs do ... end` generic iterator loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericFor {
    pub span: Span,
    pub for_token: Token,
    pub names: Punctuated<Parameter>,
    pub in_token: Token,
    pub exprs: Punctuated<Expression>,
    pub do_token: Token,
    pub block: Block,
    pub end_token: Token,
}

/// Dotted function name with optional method: `a.b.c:method`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncName {
    pub span: Span,
    pub names: Vec<Token>,
    pub dots: Vec<Token>,
    pub method: Option<(Token, Token)>,
}

/// Luau function attribute: `@native`, `@checked`, `@deprecated`, etc.
/// Attributes change runtime behavior (`@native` forces native codegen),
/// so dropping them from output is a semantics-altering bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionAttribute {
    pub span: Span,
    pub at_token: Token,
    pub name: Token,
}

/// Global function declaration: `function name(...) ... end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub span: Span,
    /// Luau: `@attr` list preceding `function`. Empty outside Luau.
    pub attributes: Vec<FunctionAttribute>,
    pub function_token: Token,
    pub name: FuncName,
    pub body: FunctionBody,
}

/// Local function declaration: `local function name(...) ... end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFunction {
    pub span: Span,
    /// Luau: `@attr` list preceding `local function`. Empty outside Luau.
    pub attributes: Vec<FunctionAttribute>,
    pub local_token: Token,
    pub function_token: Token,
    pub name: Token,
    pub body: FunctionBody,
}

/// Lua 5.4 local variable attribute: `<const>` or `<close>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub span: Span,
    pub open: Token,
    pub name: Token,
    pub close: Token,
}

/// One declared name with its optional attribute: `x <const>`.
///
/// The pairing is structural so a name and its attribute can never
/// drift apart the way parallel `names`/`attribs` vectors could.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributedName {
    pub name: Token,
    /// Luau: `: T` annotation - (colon, type). Mutually exclusive with
    /// `attrib` in practice: attributes are Lua 5.4+, annotations Luau.
    pub type_annotation: Option<(Token, Type)>,
    pub attrib: Option<Attribute>,
}

/// Local variable declaration: `local names [= exprs]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAssignment {
    pub span: Span,
    pub local_token: Token,
    pub names: Punctuated<AttributedName>,
    pub equal_and_exprs: Option<(Token, Punctuated<Expression>)>,
}

/// `goto name` statement (Lua 5.2+).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoStatement {
    pub span: Span,
    pub goto_token: Token,
    pub name: Token,
}

/// `::name::` label statement (Lua 5.2+).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelStatement {
    pub span: Span,
    pub colons_open: Token,
    pub name: Token,
    pub colons_close: Token,
}

/// `return [exprs]` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnStatement {
    pub span: Span,
    pub return_token: Token,
    pub exprs: Punctuated<Expression>,
    pub semicolon: Option<Token>,
}

/// Luau compound assignment (e.g. `x += 1`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundAssignment {
    pub span: Span,
    pub var: Var,
    pub op: Token,
    pub expr: Expression,
}

/// Luau type declaration.
/// Two forms: `type Name = TYPE` (alias) and `type function Name funcbody`
/// (compile-time type function; no `=`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDeclaration {
    pub span: Span,
    pub export_token: Option<Token>,
    pub type_token: Token,
    /// `function` keyword - present only for `type function Name funcbody`.
    pub function_token: Option<Token>,
    pub name: Token,
    pub generics: Option<Box<GenericTypeList>>,
    /// `=` - present only for the alias form.
    pub equal: Option<Token>,
    pub type_value: TypeDeclarationValue,
}

/// The right-hand side of a `type` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeDeclarationValue {
    /// `type Name = T`
    Alias(Type),
    /// `type function Name funcbody` - a compile-time function evaluated
    /// during type checking; its body is ordinary Luau.
    TypeFunction(Box<FunctionBody>),
}

/// Lua 5.5 `global` variable declaration (no initializer)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalDeclaration {
    pub span: Span,
    pub global_token: Token,
    pub names: Punctuated<AttributedName>,
}

/// Lua 5.5 `global function`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalFunction {
    pub span: Span,
    pub global_token: Token,
    pub function_token: Token,
    pub name: Token,
    pub body: FunctionBody,
}

/// Lua 5.5 `global *`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalStar {
    pub span: Span,
    pub global_token: Token,
    pub attrib: Option<Attribute>,
    pub star: Token,
}
