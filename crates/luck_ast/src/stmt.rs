use luck_token::{CompoundOp, Span, Token};

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
    EmptyStatement(Span),
    Goto(Box<GotoStatement>),
    Label(Box<LabelStatement>),
    GlobalDeclaration(Box<GlobalDeclaration>),
    GlobalFunction(Box<GlobalFunction>),
    GlobalStar(Box<GlobalStar>),
    /// Lua 5.2+: `break` as a regular statement (not just last statement)
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

/// Multi-assignment: `targets = values`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub span: Span,
    pub targets: Punctuated<Var>,
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
    pub block: Block,
}

/// `while condition do ... end` loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhileLoop {
    pub span: Span,
    pub condition: Expression,
    pub block: Block,
}

/// `repeat ... until condition` loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatLoop {
    pub span: Span,
    pub block: Block,
    pub condition: Expression,
}

/// `if ... then ... {elseif ... then ...} [else ...] end` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfStatement {
    pub span: Span,
    pub condition: Expression,
    pub block: Block,
    pub elseif_clauses: Vec<ElseIfClause>,
    pub else_clause: Option<ElseClause>,
}

/// An `elseif condition then ...` clause within an if statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseIfClause {
    pub span: Span,
    pub condition: Expression,
    pub block: Block,
}

/// An `else ...` clause within an if statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseClause {
    pub span: Span,
    pub block: Block,
}

/// `for name = start, limit [, step] do ... end` numeric loop.
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

/// `for names in exprs do ... end` generic iterator loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericFor {
    pub span: Span,
    pub names: Punctuated<Parameter>,
    pub exprs: Punctuated<Expression>,
    pub block: Block,
}

/// Dotted function name with optional method: `a.b.c:method`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncName {
    pub span: Span,
    pub names: Vec<Token>,
    /// `:method` name.
    pub method: Option<Token>,
}

/// Luau function attribute: `@native`, `@checked`, `@deprecated`, etc.
/// Attributes change runtime behavior (`@native` forces native codegen),
/// so dropping them from output is a semantics-altering bug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionAttribute {
    pub span: Span,
    pub name: Token,
    /// Literal arguments of the bracketed `@[name(...)]` form; None for
    /// the plain `@name` form (and `@[name]` without arguments).
    pub args: Option<Punctuated<Expression>>,
}

/// Global function declaration: `function name(...) ... end`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub span: Span,
    /// Luau: `@attr` list preceding `function`. Empty outside Luau.
    pub attributes: Vec<FunctionAttribute>,
    pub name: FuncName,
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
    /// Luau `const function NAME funcbody` - emitted with `const` in
    /// place of `local`.
    pub is_const: bool,
}

/// Lua 5.4 local variable attribute: `<const>` or `<close>`. The name
/// stays a token: the parser accepts any identifier there and diagnoses
/// unknown attribute names downstream with the original spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub span: Span,
    pub name: Token,
}

/// One declared name with its optional attribute: `x <const>`.
///
/// The pairing is structural so a name and its attribute can never
/// drift apart the way parallel `names`/`attribs` vectors could.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributedName {
    pub name: Token,
    /// Luau: `: T` annotation. Mutually exclusive with `attrib` in
    /// practice: attributes are Lua 5.4+, annotations Luau.
    pub type_annotation: Option<Type>,
    pub attrib: Option<Attribute>,
}

/// Local variable declaration: `local names [= exprs]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAssignment {
    pub span: Span,
    pub names: Punctuated<AttributedName>,
    pub exprs: Option<Punctuated<Expression>>,
    /// Luau `const bindinglist = explist` - emitted with `const` in
    /// place of `local`; every name in the list is read-only.
    pub is_const: bool,
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

/// `return [exprs]` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnStatement {
    pub span: Span,
    pub exprs: Punctuated<Expression>,
}

/// Luau compound assignment (e.g. `x += 1`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundAssignment {
    pub span: Span,
    pub var: Var,
    pub op: CompoundOp,
    pub expr: Expression,
}

/// Luau type declaration.
/// Two forms: `type Name = TYPE` (alias) and `type function Name funcbody`
/// (compile-time type function; no `=`). The form is carried by
/// [`TypeDeclarationValue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDeclaration {
    pub span: Span,
    pub is_exported: bool,
    pub name: Token,
    pub generics: Option<Box<GenericTypeList>>,
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

/// Lua 5.5 `global` variable declaration: `global names [= exprs]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalDeclaration {
    pub span: Span,
    pub names: Punctuated<AttributedName>,
    pub exprs: Option<Punctuated<Expression>>,
}

/// Lua 5.5 `global function`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalFunction {
    pub span: Span,
    pub name: Token,
    pub body: FunctionBody,
}

/// Lua 5.5 `global *`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalStar {
    pub span: Span,
    pub attrib: Option<Attribute>,
}
