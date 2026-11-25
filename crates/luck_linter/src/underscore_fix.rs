use luck_diagnostics::Tag;
use luck_semantic::{Scoping, Symbol};

use crate::context::ContextHost;
use crate::diagnostic::Finding;
use crate::fix::{Fix, FixKind, RuleFixer};

pub(crate) fn unused_binding(
    context: &ContextHost<'_>,
    message: String,
    symbol: &Symbol,
) -> Finding {
    Finding::new(message, symbol.definition_span)
        .with_tag(Tag::Unnecessary)
        .with_help("prefix with `_` to suppress this warning")
        .with_fix(context, FixKind::Suggestion, |fixer| {
            prefix_underscore(fixer, context.scoping(), symbol)
        })
}

/// A fix prefixing `symbol` with `_`, or `None` when a reference would stay
/// behind under the old name, or the new name is already taken.
///
/// The chunk's names are every name it spells, declared or referenced, so a
/// rename to any of them is refused, wherever it sits.
fn prefix_underscore(fixer: RuleFixer, tree: &Scoping, symbol: &Symbol) -> Option<Fix> {
    if !tree.resolved_references(symbol.id).is_empty() {
        return None;
    }
    let name = tree.name(symbol.name);
    let renamed = format!("_{name}");
    tree.name_id(&renamed).is_none().then(|| {
        fixer.replace(
            format!("prefix `{name}` with `_`"),
            symbol.definition_span,
            renamed.into_bytes(),
        )
    })
}
