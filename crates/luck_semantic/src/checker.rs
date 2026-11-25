//! The errors Luau reports beyond the grammar, each an E008, which
//! [`SemanticBuilder::with_check`](crate::SemanticBuilder::with_check) finds
//! in the walk that resolves the names they're about.

use luck_diagnostics::Diagnostic;
use luck_token::Span;

pub(crate) fn assignment_to_read_only(span: Span, name: &str) -> Diagnostic {
    Diagnostic::e008(span, format!("attempt to assign to const variable `{name}`"))
}

/// A type function runs in the type checker, where the chunk's locals don't
/// exist.
pub(crate) fn type_function_outer_local(span: Span, name: &str) -> Diagnostic {
    Diagnostic::e008(span, format!("type function cannot reference outer local `{name}`"))
}

/// `continue` skips to the `until` condition, past the locals declared after
/// it.
pub(crate) fn until_reads_skipped_local(condition: Span, name: &str) -> Diagnostic {
    Diagnostic::e008(
        condition,
        format!(
            "local `{name}` used in the `until` condition is declared after a `continue` statement"
        ),
    )
}
