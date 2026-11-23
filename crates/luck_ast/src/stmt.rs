use luck_allocator::{Box, List};
use luck_ast_macros::Ast;
use luck_token::{CompoundOp, LuaVersion, Span};

use crate::expr::{Expression, SuffixedExpression, Var};
use crate::function::{FunctionAttribute, FunctionBody, Parameter};
use crate::leaf::Name;
use crate::types::{GenericTypeList, Type};

/// A whole chunk, the root of every tree, and the dialect it's in.
///
/// Every dialect's source reads into the same shape. The version says which
/// one the tree is written in, so whatever reads or prints it takes it from
/// here.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct Chunk<'ast> {
    pub version: LuaVersion,
    pub block: Block<'ast>,
}

/// A statement that [has to end its block](Statement::has_to_end_block) in
/// the chunk's version ends it.
///
/// The parser reports one that doesn't, luck's rewrites keep it so, and the
/// printers print a block's statements as they are, so code that builds a
/// tree keeps it so too.
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct Block<'ast> {
    pub span: Span,
    pub stmts: List<'ast, Statement<'ast>>,
}

/// Two words, like [`Expression`].
///
/// A `;` isn't one. It does nothing in any dialect, as a separator in Lua
/// 5.1 and Luau and an empty statement in 5.2+, so the tree leaves it out,
/// as it does a list's commas.
#[derive(Debug, PartialEq, Eq, Ast)]
#[expect(
    clippy::enum_variant_names,
    reason = "each variant is named for its payload, and the syntax tree view prints these names"
)]
pub enum Statement<'ast> {
    Assignment(Box<'ast, Assignment<'ast>>),
    /// A chain that ends in a call.
    FunctionCall(Box<'ast, SuffixedExpression<'ast>>),
    DoBlock(Box<'ast, DoBlock<'ast>>),
    WhileLoop(Box<'ast, WhileLoop<'ast>>),
    RepeatLoop(Box<'ast, RepeatLoop<'ast>>),
    IfStatement(Box<'ast, IfStatement<'ast>>),
    NumericFor(Box<'ast, NumericFor<'ast>>),
    GenericFor(Box<'ast, GenericFor<'ast>>),
    FunctionDecl(Box<'ast, FunctionDecl<'ast>>),
    LocalFunction(Box<'ast, LocalFunction<'ast>>),
    LocalAssignment(Box<'ast, LocalAssignment<'ast>>),
    Return(Box<'ast, ReturnStatement<'ast>>),
    Break(Span),
    /// Luau: `continue`.
    Continue(Span),
    Goto(Box<'ast, GotoStatement<'ast>>),
    Label(Box<'ast, LabelStatement<'ast>>),
    GlobalDeclaration(Box<'ast, GlobalDeclaration<'ast>>),
    GlobalFunction(Box<'ast, GlobalFunction<'ast>>),
    GlobalStar(Box<'ast, GlobalStar<'ast>>),
    CompoundAssignment(Box<'ast, CompoundAssignment<'ast>>),
    TypeDeclaration(Box<'ast, TypeDeclaration<'ast>>),
    Error(Span),
}

impl Statement<'_> {
    /// Whether `version`'s grammar takes the statement only as the last of
    /// its block: a `return`, a `continue`, or a `break` where it has
    /// [`has_last_stmt_break`](LuaVersion::has_last_stmt_break).
    #[must_use]
    pub const fn has_to_end_block(&self, version: LuaVersion) -> bool {
        match self {
            Self::Return(_) | Self::Continue(_) => true,
            Self::Break(_) => version.has_last_stmt_break(),
            Self::Assignment(_)
            | Self::FunctionCall(_)
            | Self::DoBlock(_)
            | Self::WhileLoop(_)
            | Self::RepeatLoop(_)
            | Self::IfStatement(_)
            | Self::NumericFor(_)
            | Self::GenericFor(_)
            | Self::FunctionDecl(_)
            | Self::LocalFunction(_)
            | Self::LocalAssignment(_)
            | Self::Goto(_)
            | Self::Label(_)
            | Self::GlobalDeclaration(_)
            | Self::GlobalFunction(_)
            | Self::GlobalStar(_)
            | Self::CompoundAssignment(_)
            | Self::TypeDeclaration(_)
            | Self::Error(_) => false,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct Assignment<'ast> {
    pub span: Span,
    pub targets: List<'ast, Var<'ast>>,
    pub values: List<'ast, Expression<'ast>>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct DoBlock<'ast> {
    pub span: Span,
    pub block: Block<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct WhileLoop<'ast> {
    pub span: Span,
    pub condition: Expression<'ast>,
    pub block: Block<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct RepeatLoop<'ast> {
    pub span: Span,
    pub block: Block<'ast>,
    pub condition: Expression<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct IfStatement<'ast> {
    pub span: Span,
    pub condition: Expression<'ast>,
    pub block: Block<'ast>,
    pub elseif_clauses: List<'ast, ElseIfClause<'ast>>,
    pub else_clause: Option<Box<'ast, ElseClause<'ast>>>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct ElseIfClause<'ast> {
    pub span: Span,
    pub condition: Expression<'ast>,
    pub block: Block<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct ElseClause<'ast> {
    pub span: Span,
    pub block: Block<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct NumericFor<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
    pub type_annotation: Option<Box<'ast, Type<'ast>>>,
    pub start: Expression<'ast>,
    pub limit: Expression<'ast>,
    pub step: Option<Expression<'ast>>,
    pub block: Block<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct GenericFor<'ast> {
    pub span: Span,
    pub names: List<'ast, Parameter<'ast>>,
    pub exprs: List<'ast, Expression<'ast>>,
    pub block: Block<'ast>,
}

/// `a.b.c` or `a.b:c`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct FunctionName<'ast> {
    pub span: Span,
    pub names: List<'ast, Name<'ast>>,
    pub method: Option<Name<'ast>>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct FunctionDecl<'ast> {
    pub span: Span,
    /// Luau: `@attr` list preceding `function`. Empty outside Luau.
    pub attributes: List<'ast, FunctionAttribute<'ast>>,
    pub name: FunctionName<'ast>,
    pub body: FunctionBody<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct LocalFunction<'ast> {
    pub span: Span,
    /// Luau: `@attr` list preceding `local function`. Empty outside Luau.
    pub attributes: List<'ast, FunctionAttribute<'ast>>,
    pub name: Name<'ast>,
    pub body: FunctionBody<'ast>,
    pub kind: LocalFunctionKind,
}

/// The keyword a [`LocalFunction`] is declared with, which fixes whether its
/// binding is read-only and whether the module exports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub enum LocalFunctionKind {
    Local,
    /// Luau: `const function`.
    Const,
    /// Luau: `export function`, whose binding is read-only too.
    Export,
}

impl LocalFunctionKind {
    #[must_use]
    pub const fn is_const(self) -> bool {
        match self {
            Self::Local => false,
            Self::Const | Self::Export => true,
        }
    }

    #[must_use]
    pub const fn is_exported(self) -> bool {
        match self {
            Self::Local | Self::Const => false,
            Self::Export => true,
        }
    }
}

/// Lua 5.4+: `<const>` or `<close>`. The name is whatever identifier the
/// parser finds there, after it reports any other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub struct Attribute<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct AttributedName<'ast> {
    pub name: Name<'ast>,
    /// Luau: `: T` annotation. The parser never sets both this and
    /// `attribute`, since attributes are Lua 5.4+ only.
    pub type_annotation: Option<Box<'ast, Type<'ast>>>,
    pub attribute: Option<Box<'ast, Attribute<'ast>>>,
}

/// `exprs` is empty for a declaration with no `=`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct LocalAssignment<'ast> {
    pub span: Span,
    pub names: List<'ast, AttributedName<'ast>>,
    pub exprs: List<'ast, Expression<'ast>>,
    pub kind: LocalAssignmentKind,
}

/// The keywords a [`LocalAssignment`] is declared with, which fix whether its
/// names are read-only and whether the module exports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub enum LocalAssignmentKind {
    Local,
    /// Luau: `const`, which makes every name in the list read-only.
    Const,
    /// Luau: `export local`.
    ExportLocal,
    /// Luau: `export const`.
    ExportConst,
}

impl LocalAssignmentKind {
    #[must_use]
    pub const fn is_const(self) -> bool {
        match self {
            Self::Local | Self::ExportLocal => false,
            Self::Const | Self::ExportConst => true,
        }
    }

    #[must_use]
    pub const fn is_exported(self) -> bool {
        match self {
            Self::Local | Self::Const => false,
            Self::ExportLocal | Self::ExportConst => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub struct GotoStatement<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub struct LabelStatement<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
}

/// The span takes in a `;` that ends it, which Lua 5.2+ reads as part of a
/// `return`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct ReturnStatement<'ast> {
    pub span: Span,
    pub exprs: List<'ast, Expression<'ast>>,
}

/// Luau: `var op= expr`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct CompoundAssignment<'ast> {
    pub span: Span,
    pub var: Var<'ast>,
    pub op: CompoundOp,
    pub expr: Expression<'ast>,
}

/// Luau: `type Name<T> = T` or `type function Name funcbody`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct TypeDeclaration<'ast> {
    pub span: Span,
    pub is_exported: bool,
    pub name: Name<'ast>,
    pub generics: Option<Box<'ast, GenericTypeList<'ast>>>,
    pub value: TypeDeclarationValue<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub enum TypeDeclarationValue<'ast> {
    Alias(Type<'ast>),
    /// `type function Name funcbody`, a compile-time function evaluated
    /// during type checking. Its body is ordinary Luau.
    TypeFunction(Box<'ast, FunctionBody<'ast>>),
}

/// Lua 5.5: `global names [= exprs]`. `exprs` is empty when there's no `=`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct GlobalDeclaration<'ast> {
    pub span: Span,
    pub names: List<'ast, AttributedName<'ast>>,
    pub exprs: List<'ast, Expression<'ast>>,
}

/// Lua 5.5: `global function Name funcbody`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct GlobalFunction<'ast> {
    pub span: Span,
    pub name: Name<'ast>,
    pub body: FunctionBody<'ast>,
}

/// Lua 5.5: `global *`, with an optional attribute before the `*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub struct GlobalStar<'ast> {
    pub span: Span,
    pub attribute: Option<Attribute<'ast>>,
}
