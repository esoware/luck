//! Shared rendering of stdlib parameter labels, used by the hover,
//! completion, and signature-help providers so a parameter reads the same
//! everywhere it is shown.

use luck_semantic::stdlib_model::{StdlibArgKind, StdlibParam};

/// The bare type word for an argument kind (`"number"`, `"string"`, ...).
/// Varargs render as `"..."`; constrained constants render as `"constant"`
/// (callers that want the value set render it themselves).
#[must_use]
pub(crate) fn arg_type_word(kind: &StdlibArgKind) -> &str {
    match kind {
        StdlibArgKind::Any => "any",
        StdlibArgKind::Bool => "bool",
        StdlibArgKind::Number => "number",
        StdlibArgKind::String => "string",
        StdlibArgKind::Function => "function",
        StdlibArgKind::Table => "table",
        StdlibArgKind::Nil => "nil",
        StdlibArgKind::Display(display) => display.as_str(),
        StdlibArgKind::Constant(_) => "constant",
        StdlibArgKind::Vararg => "...",
    }
}

/// A parameter's label: its type word, suffixed with `?` when optional.
/// Varargs are always `"..."` (never suffixed).
#[must_use]
pub(crate) fn param_label(param: &StdlibParam) -> String {
    let word = arg_type_word(&param.kind);
    if param.required || matches!(param.kind, StdlibArgKind::Vararg) {
        word.to_string()
    } else {
        format!("{word}?")
    }
}
