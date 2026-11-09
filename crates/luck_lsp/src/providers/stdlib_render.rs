use luck_semantic::{StdlibArgKind, StdlibConstant, StdlibParam};

/// How many values of a constant set hover and signature help list. Large
/// generated sets (Roblox service and class names) would swamp the panel, so
/// they show a prefix and the total.
const CONSTANT_PREVIEW_LIMIT: usize = 8;

/// Varargs are always `"..."` (never suffixed), and constrained constants
/// render as `"constant"` (callers that want the value set add
/// [`constant_preview`]).
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

/// The first [`CONSTANT_PREVIEW_LIMIT`] of `values`, each rendered by
/// `render` and joined by `separator`, ending in the total when any are cut.
pub(crate) fn constant_preview(
    values: &[StdlibConstant],
    separator: &str,
    render: impl Fn(&StdlibConstant) -> String,
) -> String {
    let preview =
        values.iter().take(CONSTANT_PREVIEW_LIMIT).map(render).collect::<Vec<_>>().join(separator);
    if values.len() > CONSTANT_PREVIEW_LIMIT {
        format!("{preview}{separator}... ({} values)", values.len())
    } else {
        preview
    }
}
