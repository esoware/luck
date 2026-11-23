//! The compile-time errors Lua and Luau report beyond the grammar, each an
//! E008, which [`SemanticBuilder::with_check`](crate::SemanticBuilder::with_check)
//! finds in the walk that resolves the names they're about.

use luck_diagnostics::Diagnostic;
use luck_token::Span;

pub(crate) fn assignment_to_read_only(span: Span, name: &str) -> Diagnostic {
    Diagnostic::e008(span, format!("attempt to assign to const variable `{name}`"))
}

/// Lua 5.5: a free name while a `global` declaration is in scope and no
/// `global *` is.
pub(crate) fn undeclared_global(span: Span, name: &str) -> Diagnostic {
    Diagnostic::e008(span, format!("variable `{name}` not declared"))
}

/// Luau: a type function runs in the type checker, where the chunk's locals
/// don't exist.
pub(crate) fn type_function_outer_local(span: Span, name: &str) -> Diagnostic {
    Diagnostic::e008(span, format!("type function cannot reference outer local `{name}`"))
}

/// Luau: `continue` skips to the `until` condition, past the locals declared
/// after it.
pub(crate) fn until_reads_skipped_local(condition: Span, name: &str) -> Diagnostic {
    Diagnostic::e008(
        condition,
        format!(
            "local `{name}` used in the `until` condition is declared after a `continue` statement"
        ),
    )
}

pub(crate) fn duplicate_label(span: Span, name: &str, first: Span) -> Diagnostic {
    Diagnostic::e008(span, format!("label `{name}` already defined"))
        .with_label(first, "first defined here")
}

pub(crate) fn goto_into_local_scope(span: Span, name: &str) -> Diagnostic {
    Diagnostic::e008(span, format!("`goto {name}` jumps into the scope of a local"))
}

pub(crate) fn unresolved_goto(span: Span, name: &str) -> Diagnostic {
    Diagnostic::e008(span, format!("no visible label `{name}` for `goto`"))
}
