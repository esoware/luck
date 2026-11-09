use luck_ast::{Expression, FunctionArgs, FunctionCall, Var, is_multi_value};
use luck_semantic::SemanticAnalysis;
use luck_token::{Span, Token, short_string_body};

/// Whether `call` is a plain `name(...)` call to the host's global `name`. A
/// method call, or a `name` the chunk shadows or rebinds, is user code.
pub(crate) fn is_global_call(call: &FunctionCall, name: &str, semantic: &SemanticAnalysis) -> bool {
    call.method.is_none()
        && matches!(
            &call.callee,
            Expression::Var(Var::Name(token))
                if token.identifier() == Some(name)
                    && semantic.reaches_host_global(name, token.span)
        )
}

/// `None` for a method call.
pub(crate) fn dotted_callee(call: &FunctionCall) -> Option<(&Token, &str)> {
    if call.method.is_some() {
        return None;
    }
    let Expression::Var(Var::FieldAccess(field_access)) = &call.callee else {
        return None;
    };
    let Expression::Var(Var::Name(root)) = &field_access.prefix else {
        return None;
    };
    Some((root, field_access.name.identifier()?))
}

/// `f"..."` and `f{...}` pass one argument, which isn't in the list.
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
