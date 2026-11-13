use luck_token::{BinOp, CompactString, Span, Token, UnOp};

use crate::function::{FunctionAttribute, FunctionBody};
use crate::one_or_more::OneOrMore;
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
    Name(Token),
    Suffixed(Box<SuffixedExpression>),
    Parenthesized(Box<ParenthesizedExpression>),
    TableConstructor(Box<TableConstructor>),
    BinaryOp(Box<BinaryOp>),
    UnaryOp(Box<UnaryOp>),
    IfExpression(Box<IfExpression>),
    InterpolatedString(Box<InterpolatedString>),
    TypeCast(Box<TypeCast>),
    Error(Span),
}

/// The owning variant implies the token kind, so none is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Literal {
    pub text: CompactString,
    pub span: Span,
}

/// An assignment target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Var {
    Name(Token),
    /// A chain whose last suffix is a field or an index.
    Suffixed(Box<SuffixedExpression>),
}

/// A name or a parenthesized expression and the run of fields, indexes,
/// calls, and Luau instantiations after it, as one node however long the
/// run: `a.b[c]:d(e)` is `a` and four suffixes.
///
/// The last suffix decides what the whole is: a call, or a [`Var`] that can
/// be assigned to.
///
/// `prefix` is never itself a suffixed expression, so a run adds one level of
/// nesting whatever its length. The parser and `Synth` keep it that way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuffixedExpression {
    pub span: Span,
    pub prefix: Expression,
    pub suffixes: OneOrMore<Suffix>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suffix {
    /// A field, `.name`.
    Field(Token),
    /// `[index]`, where `span` covers the brackets.
    Index {
        span: Span,
        index: Expression,
    },
    Call(CallSuffix),
    /// Luau: `<<T>>`, where `span` covers the outer angle brackets.
    TypeInstantiation {
        span: Span,
        type_args: TypeArgs,
    },
}

/// `(args)`, `{...}`, or `"..."`, or with `method`, `:name` before them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSuffix {
    pub method: Option<Token>,
    /// Luau: explicit type arguments between a method name and its call
    /// arguments (`receiver:method<<T>>()`). Non-method instantiation is a
    /// [`Suffix::TypeInstantiation`] instead.
    pub explicit_type_args: Option<Box<TypeArgs>>,
    pub args: FunctionArgs,
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

/// A run of binary operators that share one precedence level, as one node
/// however long the run: `a + b - c` is `a` and two links.
///
/// A left-associative level groups from the left, so that run is
/// `(a + b) - c`. The right-associative `..` and `^` group from the right, so
/// `a .. b .. c` is `a .. (b .. c)`.
///
/// An operand is never a bare run at the same level where it would merge
/// into this one when printed, since that is what reading the printed run
/// back gives. The parser and `Synth` keep it that way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryOp {
    pub span: Span,
    pub first: Expression,
    /// Every operator here shares one precedence level.
    pub links: OneOrMore<BinaryLink>,
}

/// An operator and the operand after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryLink {
    pub op: BinOp,
    pub operand: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnaryOp {
    pub span: Span,
    pub op: UnOp,
    pub operand: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDef {
    pub span: Span,
    /// Luau: `@attr` list preceding `function`. Empty outside Luau.
    pub attributes: Vec<FunctionAttribute>,
    pub body: FunctionBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfExpression {
    pub span: Span,
    pub condition: Expression,
    pub then_expr: Expression,
    pub elseif_clauses: Vec<ElseIfExprClause>,
    pub else_expr: Expression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseIfExprClause {
    pub span: Span,
    pub condition: Expression,
    pub expr: Expression,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeCast {
    pub span: Span,
    pub expr: Expression,
    pub type_annotation: Type,
}
