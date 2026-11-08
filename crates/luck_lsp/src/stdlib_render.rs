//! Stdlib parameter rendering, shared so hover, completion, and signature
//! help render a parameter the same way.

use luck_semantic::{StdlibArgKind, StdlibParam};

/// How many values of a constant set hover and signature help list. Large
/// generated sets (Roblox service and class names) would swamp the panel, so
/// they show a prefix and the total.
pub(crate) const CONSTANT_PREVIEW_LIMIT: usize = 8;

/// A parameter's label: its type word, suffixed with `?` when optional.
///
/// Varargs are always `"..."` (never suffixed), and constrained constants
/// render as `"constant"` (callers that want the value set render it
/// themselves).
pub(crate) fn param_label(param: &StdlibParam) -> String {
    let word = match &param.kind {
        StdlibArgKind::Any => "any",
        StdlibArgKind::Bool => "bool",
        StdlibArgKind::Number => "number",
        StdlibArgKind::String => "string",
        StdlibArgKind::Function => "function",
        StdlibArgKind::Table => "table",
        StdlibArgKind::Display(display) => display.as_str(),
        StdlibArgKind::Constant(_) => "constant",
        StdlibArgKind::Vararg => return "...".to_owned(),
    };
    if param.is_required { word.to_owned() } else { format!("{word}?") }
}
