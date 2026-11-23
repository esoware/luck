use core::slice;

use luck_ast::{
    Call, Expression, FunctionArgs, GetSpan as _, Name, StringLiteral, Suffix, TableConstructor,
    is_multi_value,
};
use luck_stdlib::SemanticAnalysis;
use luck_token::{LuaVersion, Span};
use luck_value::ToBoolean as _;

/// Whether `call` is a plain `name(...)` call to the host's global `name`. A
/// method call, or a `name` the chunk shadows or rebinds, is user code.
pub(crate) fn is_global_call(call: Call<'_>, name: &str, semantic: &SemanticAnalysis<'_>) -> bool {
    call.method.is_none()
        && call.callee.as_name().is_some_and(|callee| {
            callee.text == name && semantic.scoping().reaches_host_global(callee)
        })
}

/// The root and the field of a `root.field(...)` call. `None` for a method
/// call.
pub(crate) fn dotted_callee(call: Call<'_>) -> Option<(&Name<'_>, &str)> {
    if call.method.is_some() {
        return None;
    }
    let (Expression::Name(root), [Suffix::Field(field)]) =
        (call.callee.prefix, call.callee.suffixes)
    else {
        return None;
    };
    (!field.text.is_empty()).then_some((root, field.text.as_str()))
}

/// An argument as a call passes it. `f"..."` and `f{...}` pass theirs with
/// no list around it, and mean what `f("...")` and `f({...})` do.
#[derive(Debug, Clone, Copy)]
pub(crate) enum CallArg<'ast> {
    Expr(&'ast Expression<'ast>),
    String(&'ast StringLiteral<'ast>),
    Table(&'ast TableConstructor<'ast>),
}

impl<'ast> CallArg<'ast> {
    pub(crate) fn span(self) -> Span {
        match self {
            Self::Expr(expr) => expr.span(),
            Self::String(literal) => literal.span,
            Self::Table(table) => table.span,
        }
    }

    pub(crate) fn string_literal(self) -> Option<&'ast StringLiteral<'ast>> {
        match self {
            Self::Expr(Expression::StringLiteral(literal)) => Some(literal),
            Self::String(literal) => Some(literal),
            Self::Expr(_) | Self::Table(_) => None,
        }
    }

    /// Whether it's a call or `...`, which as the last argument can expand
    /// to any number of values, none included.
    pub(crate) fn is_multi_value(self) -> bool {
        matches!(self, Self::Expr(expr) if is_multi_value(expr))
    }

    /// Whether the argument's value is truthy, when that's known.
    pub(crate) fn to_boolean(self, version: LuaVersion) -> Option<bool> {
        match self {
            Self::Expr(expr) => expr.to_boolean(&version),
            Self::String(_) | Self::Table(_) => Some(true),
        }
    }
}

/// Every argument a call passes, first to last.
#[derive(Debug, Clone)]
pub(crate) struct CallArgs<'ast> {
    list: slice::Iter<'ast, Expression<'ast>>,
    /// The one argument of `f"..."` or `f{...}`, which have no list.
    bare: Option<CallArg<'ast>>,
}

impl<'ast> CallArgs<'ast> {
    pub(crate) fn new(function_args: &'ast FunctionArgs<'ast>) -> Self {
        let (list, bare): (&[Expression<'_>], _) = match function_args {
            FunctionArgs::Parenthesized { args, .. } => (args, None),
            FunctionArgs::StringLiteral(literal) => (&[], Some(CallArg::String(literal))),
            FunctionArgs::TableConstructor(table) => (&[], Some(CallArg::Table(table))),
            FunctionArgs::Error(_) => (&[], None),
        };
        Self { list: list.iter(), bare }
    }
}

impl<'ast> Iterator for CallArgs<'ast> {
    type Item = CallArg<'ast>;

    fn next(&mut self) -> Option<CallArg<'ast>> {
        self.list.next().map(CallArg::Expr).or_else(|| self.bare.take())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let count = self.list.len().saturating_add(usize::from(self.bare.is_some()));
        (count, Some(count))
    }
}

impl ExactSizeIterator for CallArgs<'_> {}

/// A string literal argument's value, when it's UTF-8, and where the literal
/// is.
pub(crate) fn string_literal_value(arg: CallArg<'_>) -> Option<(&str, Span)> {
    let literal = arg.string_literal()?;
    Some((literal.as_str()?, literal.span))
}
