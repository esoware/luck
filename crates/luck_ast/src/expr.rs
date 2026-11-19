use luck_allocator::{Box, List};
use luck_ast_macros::Ast;
use luck_token::{BinOp, Span, UnOp};

use crate::function::{FunctionAttribute, FunctionBody};
use crate::leaf::{IntegerLiteral, Name, NumberLiteral, StringLiteral};
use crate::one_or_more::OneOrMore;
use crate::types::{Type, TypeArgs};

/// Two words: every variant is a span or a box. `nil`, `true`, `false`,
/// `...`, and an error are their span alone.
#[derive(Debug, PartialEq, Eq, Ast)]
#[expect(
    clippy::enum_variant_names,
    reason = "each variant is named for its payload, and the syntax tree view prints these names"
)]
pub enum Expression<'ast> {
    Nil(Span),
    False(Span),
    True(Span),
    Number(Box<'ast, NumberLiteral<'ast>>),
    /// Luau: exact 64-bit integer literal with an `i` suffix.
    Integer(Box<'ast, IntegerLiteral<'ast>>),
    StringLiteral(Box<'ast, StringLiteral<'ast>>),
    VarArg(Span),
    FunctionDef(Box<'ast, FunctionDef<'ast>>),
    Name(Box<'ast, Name<'ast>>),
    Suffixed(Box<'ast, SuffixedExpression<'ast>>),
    Parenthesized(Box<'ast, ParenthesizedExpression<'ast>>),
    TableConstructor(Box<'ast, TableConstructor<'ast>>),
    BinaryOp(Box<'ast, BinaryOp<'ast>>),
    UnaryOp(Box<'ast, UnaryOp<'ast>>),
    IfExpression(Box<'ast, IfExpression<'ast>>),
    InterpolatedString(Box<'ast, InterpolatedString<'ast>>),
    TypeCast(Box<'ast, TypeCast<'ast>>),
    Error(Span),
}

/// An assignment target.
#[derive(Debug, PartialEq, Eq, Ast)]
pub enum Var<'ast> {
    Name(Box<'ast, Name<'ast>>),
    /// A chain whose last suffix is a field or an index.
    Suffixed(Box<'ast, SuffixedExpression<'ast>>),
}

/// A name or a parenthesized expression and the run of fields, indexes,
/// calls, and Luau instantiations after it, as one node however long the
/// run: `a.b[c]:d(e)` is `a` and three suffixes.
///
/// The last suffix decides what the whole is: a call, or a [`Var`] that can
/// be assigned to.
///
/// `prefix` is never itself a suffixed expression, so a run adds one level of
/// nesting whatever its length. The parser and `Synth` keep it that way, and
/// [`SuffixedExpression::splice_prefix`] restores it after a rewrite.
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct SuffixedExpression<'ast> {
    pub span: Span,
    pub prefix: Expression<'ast>,
    pub suffixes: OneOrMore<'ast, Suffix<'ast>>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub enum Suffix<'ast> {
    /// `.name`.
    Field(Name<'ast>),
    /// `[index]`, where `span` covers the brackets.
    Index { span: Span, index: Expression<'ast> },
    /// `(args)`, `{...}`, or `"..."`.
    Call(FunctionArgs<'ast>),
    /// `:name(args)`.
    MethodCall(Box<'ast, MethodCall<'ast>>),
    /// Luau: `<<T>>`, where `span` covers the outer angle brackets.
    TypeInstantiation { span: Span, type_args: Box<'ast, TypeArgs<'ast>> },
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct MethodCall<'ast> {
    pub name: Name<'ast>,
    /// Luau: `receiver:method<<T>>()`. A plain call's instantiation is a
    /// [`Suffix::TypeInstantiation`] instead.
    pub type_args: Option<Box<'ast, TypeArgs<'ast>>>,
    pub args: FunctionArgs<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub enum FunctionArgs<'ast> {
    /// `span` covers the parentheses.
    Parenthesized {
        span: Span,
        args: List<'ast, Expression<'ast>>,
    },
    TableConstructor(Box<'ast, TableConstructor<'ast>>),
    StringLiteral(Box<'ast, StringLiteral<'ast>>),
    /// Arguments the parser found missing, or a table argument the depth cap
    /// stopped.
    Error(Span),
}

/// A run of binary operators that share one precedence level, as one node
/// however long the run: `a + b - c` is `a` and two links.
///
/// A left-associative level groups from the left, so that run is
/// `(a + b) - c`. The right-associative `..` and `^` group from the right, so
/// `a .. b .. c` is `a .. (b .. c)`.
///
/// An operand is never a bare run at the same level where it would merge
/// into this one when printed, since that is what reading the printed run
/// back gives. The parser and `Synth` keep it that way, and
/// [`BinaryOp::splice_operands`] restores it after a rewrite.
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct BinaryOp<'ast> {
    pub span: Span,
    pub first: Expression<'ast>,
    pub links: OneOrMore<'ast, BinaryLink<'ast>>,
}

/// An operator and the operand after it.
#[derive(Debug, PartialEq, Eq, Ast)]
pub struct BinaryLink<'ast> {
    pub op: BinOp,
    pub operand: Expression<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct UnaryOp<'ast> {
    pub span: Span,
    pub op: UnOp,
    pub operand: Expression<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct ParenthesizedExpression<'ast> {
    pub span: Span,
    pub expr: Expression<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct TableConstructor<'ast> {
    pub span: Span,
    pub fields: List<'ast, Field<'ast>>,
    /// Whether a `,` or `;` follows the last field. The printers write `,`.
    #[content_eq(skip)]
    pub has_trailing_separator: bool,
}

/// A named or positional field spans from its name or value, so it holds no
/// span of its own.
#[derive(Debug, PartialEq, Eq, Ast)]
pub enum Field<'ast> {
    /// `[key] = value`, where `span` covers the whole field.
    Bracketed {
        span: Span,
        key: Expression<'ast>,
        value: Expression<'ast>,
    },
    Named {
        name: Name<'ast>,
        value: Expression<'ast>,
    },
    Positional(Expression<'ast>),
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct FunctionDef<'ast> {
    pub span: Span,
    /// Luau: `@attr` list preceding `function`. Empty outside Luau.
    pub attributes: List<'ast, FunctionAttribute<'ast>>,
    pub body: FunctionBody<'ast>,
}

/// Luau: `if condition then expr elseif ... else expr`.
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct IfExpression<'ast> {
    pub span: Span,
    pub condition: Expression<'ast>,
    pub then_expr: Expression<'ast>,
    pub elseif_clauses: List<'ast, ElseIfExprClause<'ast>>,
    pub else_expr: Expression<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct ElseIfExprClause<'ast> {
    pub span: Span,
    pub condition: Expression<'ast>,
    pub expr: Expression<'ast>,
}

/// Luau: `` `text{expr}text` ``. Each piece of text spans only itself, not
/// its delimiters, and its spelling leaves them out too.
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct InterpolatedString<'ast> {
    pub span: Span,
    /// Each expression, with the text before it.
    pub segments: List<'ast, InterpolatedSegment<'ast>>,
    /// The text after the last expression, or all of it when there's none.
    pub tail: StringLiteral<'ast>,
}

#[derive(Debug, PartialEq, Eq, Ast)]
pub struct InterpolatedSegment<'ast> {
    pub text: StringLiteral<'ast>,
    pub expr: Expression<'ast>,
}

/// Luau: `expr :: T`.
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct TypeCast<'ast> {
    pub span: Span,
    pub expr: Expression<'ast>,
    pub ty: Type<'ast>,
}
