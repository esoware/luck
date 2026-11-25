use std::mem;

use luck_allocator::{Box, List};
use luck_ast_macros::Ast;
use luck_token::{CompoundOp, Span};

use crate::expr::{Expression, SuffixedExpression, Var};
use crate::function::{FunctionAttribute, FunctionBody};
use crate::leaf::Name;
use crate::types::{GenericTypeList, Type};

/// A statement that [has to end its block](Statement::has_to_end_block) ends
/// it.
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

impl Block<'_> {
    /// Moves the value out of a `&mut` and leaves an empty block behind, as
    /// [`Expression::take`] does.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self::new(Span::new(0, 0), List::new()))
    }
}

/// Two words, like [`Expression`].
///
/// A `;` isn't one. It only separates statements, so the tree leaves it out,
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
    Continue(Span),
    CompoundAssignment(Box<'ast, CompoundAssignment<'ast>>),
    TypeDeclaration(Box<'ast, TypeDeclaration<'ast>>),
    Error(Span),
}

impl<'ast> Statement<'ast> {
    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind, as [`Expression::take`] does.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::new(0, 0)))
    }

    /// The prefix a call or an assignment starts with, inside any chain it
    /// starts.
    ///
    /// A printer that writes it in parentheses starts the statement with `(`,
    /// which glues onto a statement before it that ends in an expression and
    /// reads back as a call, so it puts a `;` between them.
    pub fn leading_prefix(&self) -> Option<&Expression<'ast>> {
        let mut current = match self {
            Self::FunctionCall(chain) => &chain.prefix,
            Self::Assignment(assignment) => match assignment.targets.first()? {
                Var::Name(_) => return None,
                Var::Suffixed(chain) => &chain.prefix,
            },
            Self::CompoundAssignment(compound) => match &compound.var {
                Var::Name(_) => return None,
                Var::Suffixed(chain) => &chain.prefix,
            },
            Self::DoBlock(_)
            | Self::WhileLoop(_)
            | Self::RepeatLoop(_)
            | Self::IfStatement(_)
            | Self::NumericFor(_)
            | Self::GenericFor(_)
            | Self::FunctionDecl(_)
            | Self::LocalFunction(_)
            | Self::LocalAssignment(_)
            | Self::Return(_)
            | Self::Break(_)
            | Self::Continue(_)
            | Self::TypeDeclaration(_)
            | Self::Error(_) => return None,
        };
        while let Expression::Suffixed(chain) = current {
            current = &chain.prefix;
        }
        Some(current)
    }

    /// Whether the grammar takes the statement only as the last of its block:
    /// a `return`, a `break`, or a `continue`.
    pub const fn has_to_end_block(&self) -> bool {
        match self {
            Self::Return(_) | Self::Break(_) | Self::Continue(_) => true,
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
    pub variable: TypedName<'ast>,
    pub start: Expression<'ast>,
    pub limit: Expression<'ast>,
    pub step: Option<Expression<'ast>>,
    pub block: Block<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct GenericFor<'ast> {
    pub span: Span,
    pub names: List<'ast, TypedName<'ast>>,
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
    /// The `@attr` list preceding `function`.
    pub attributes: List<'ast, FunctionAttribute<'ast>>,
    pub name: FunctionName<'ast>,
    pub body: FunctionBody<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct LocalFunction<'ast> {
    pub span: Span,
    /// The `@attr` list preceding `local function`.
    pub attributes: List<'ast, FunctionAttribute<'ast>>,
    pub name: Name<'ast>,
    pub body: FunctionBody<'ast>,
    /// Where the `local`, `const`, or `export` keyword `kind` names starts,
    /// which is where `span` does unless an attribute comes first.
    #[content_eq(skip)]
    pub keyword_start: u32,
    pub kind: LocalFunctionKind,
}

/// The keyword a [`LocalFunction`] is declared with, which fixes whether its
/// binding is read-only and whether the module exports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub enum LocalFunctionKind {
    Local,
    /// `const function`.
    Const,
    /// `export function`, whose binding is read-only too.
    Export,
}

impl LocalFunctionKind {
    pub const fn is_const(self) -> bool {
        match self {
            Self::Local => false,
            Self::Const | Self::Export => true,
        }
    }

    pub const fn is_exported(self) -> bool {
        match self {
            Self::Local | Self::Const => false,
            Self::Export => true,
        }
    }
}

/// A name a function parameter, a `local`, or a `for` declares, with its
/// `: T` annotation. It spans from its name, so it holds no span of its own.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct TypedName<'ast> {
    pub name: Name<'ast>,
    pub type_annotation: Option<Box<'ast, Type<'ast>>>,
}

/// `exprs` is empty for a declaration with no `=`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct LocalAssignment<'ast> {
    pub span: Span,
    pub names: List<'ast, TypedName<'ast>>,
    pub exprs: List<'ast, Expression<'ast>>,
    pub kind: LocalAssignmentKind,
}

/// The keywords a [`LocalAssignment`] is declared with, which fix whether its
/// names are read-only and whether the module exports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub enum LocalAssignmentKind {
    Local,
    /// `const`, which makes every name in the list read-only.
    Const,
    /// `export local`.
    ExportLocal,
    /// `export const`.
    ExportConst,
}

impl LocalAssignmentKind {
    pub const fn is_const(self) -> bool {
        match self {
            Self::Local | Self::ExportLocal => false,
            Self::Const | Self::ExportConst => true,
        }
    }

    pub const fn is_exported(self) -> bool {
        match self {
            Self::Local | Self::Const => false,
            Self::ExportLocal | Self::ExportConst => true,
        }
    }
}

/// The span takes in a `;` that ends it.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct ReturnStatement<'ast> {
    pub span: Span,
    pub exprs: List<'ast, Expression<'ast>>,
}

/// `var op= expr`.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct CompoundAssignment<'ast> {
    pub span: Span,
    pub var: Var<'ast>,
    pub op: CompoundOp,
    pub expr: Expression<'ast>,
}

/// `type Name<T> = T` or `type function Name funcbody`.
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
