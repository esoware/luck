//! Helpers for the rules that match a call: which global it calls, and what
//! it passes.

use luck_ast::{Expression, FunctionArgs, FunctionCall, Var, is_multi_value};
use luck_semantic::SemanticAnalysis;
use luck_token::{Span, short_string_body};

/// Whether `call` is a plain `name(...)` call to the global `name`. A method
/// call, or a local that shadows `name`, is user code.
pub(crate) fn is_global_call(call: &FunctionCall, name: &str, semantic: &SemanticAnalysis) -> bool {
    call.method.is_none()
        && matches!(
            &call.callee,
            Expression::Var(Var::Name(token))
                if token.identifier() == Some(name)
                    && !semantic.resolves_to_local(name, token.span)
        )
}

/// A call's positional argument expressions, and how many arguments it
/// passes. `f"..."` and `f{...}` pass one argument, which isn't in the list.
pub(crate) const fn positional_args(function_args: &FunctionArgs) -> (&[Expression], usize) {
    match function_args {
        FunctionArgs::Parenthesized { args, .. } => (args.items.as_slice(), args.len()),
        FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => (&[], 1),
    }
}

/// Whether the last argument, or the last value of an assignment, is a call
/// or `...`, which can expand to any number of values, none included.
pub(crate) const fn has_multi_value_tail(args: &[Expression]) -> bool {
    matches!(args.last(), Some(last) if is_multi_value(last))
}

/// The text between a quoted string argument's quotes, and its span.
///
/// Escapes stay undecoded, so a value spelled with escapes won't match a
/// catalog constant. Long-bracket strings return `None`.
pub(crate) fn string_literal_value(expr: &Expression) -> Option<(&str, Span)> {
    let Expression::StringLiteral(literal) = expr else {
        return None;
    };
    let body = short_string_body(&literal.text)?;
    let body_span =
        Span::new(literal.span.start.saturating_add(1), literal.span.end.saturating_sub(1));
    Some((body, body_span))
}
