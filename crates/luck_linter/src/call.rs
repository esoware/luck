use luck_ast::{Call, Expression, FunctionArgs, Name, Suffix, is_multi_value};
use luck_semantic::SemanticAnalysis;
use luck_token::{Span, short_string_body};

/// Whether `call` is a plain `name(...)` call to the host's global `name`. A
/// method call, or a `name` the chunk shadows or rebinds, is user code.
pub(crate) fn is_global_call(call: Call<'_>, name: &str, semantic: &SemanticAnalysis<'_>) -> bool {
    call.method.is_none()
        && call.callee.as_name().is_some_and(|callee| {
            callee.text == name && semantic.scope_tree().reaches_host_global(callee)
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
    (!field.text.is_empty()).then_some((root, field.text))
}

/// `f"..."` and `f{...}` pass one argument, which isn't in the list.
pub(crate) fn positional_args<'ast>(
    function_args: &'ast FunctionArgs<'ast>,
) -> (&'ast [Expression<'ast>], usize) {
    match function_args {
        FunctionArgs::Parenthesized { args, .. } => (args, args.len()),
        FunctionArgs::TableConstructor(_) | FunctionArgs::StringLiteral(_) => (&[], 1),
        FunctionArgs::Error(_) => (&[], 0),
    }
}

/// Whether the last argument, or the last value of an assignment, is a call
/// or `...`, which can expand to any number of values, none included.
pub(crate) fn has_multi_value_tail(args: &[Expression<'_>]) -> bool {
    args.last().is_some_and(is_multi_value)
}

/// Escapes stay undecoded, so a value spelled with escapes won't match a
/// catalog constant. Long-bracket strings return `None`.
pub(crate) fn string_literal_value<'ast>(expr: &Expression<'ast>) -> Option<(&'ast str, Span)> {
    let Expression::StringLiteral(literal) = expr else {
        return None;
    };
    let body = short_string_body(literal.text)?;
    let body_span =
        Span::new(literal.span.start.saturating_add(1), literal.span.end.saturating_sub(1));
    Some((body, body_span))
}
