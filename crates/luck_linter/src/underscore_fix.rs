use luck_diagnostics::Tag;
use luck_semantic::Symbol;
use luck_stdlib::SemanticAnalysis;

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
            prefix_underscore(fixer, context.semantic, symbol)
        })
}

/// A fix prefixing `symbol` with `_`, or `None` when a reference would stay
/// behind under the old name, or the new name is already taken.
///
/// The chunk's names are every name it spells, declared or referenced, so a
/// rename to any of them is refused, wherever it sits.
fn prefix_underscore(
    fixer: RuleFixer,
    semantic: &SemanticAnalysis<'_>,
    symbol: &Symbol,
) -> Option<Fix> {
    let tree = semantic.scoping();
    if !tree.resolved_references(symbol.id).is_empty() {
        return None;
    }
    let name = tree.name(symbol.name);
    let renamed = format!("_{name}");
    // Lua 5.2+: a binding named `_ENV` redirects every global in scope.
    if semantic.version().has_env_upvalue() && renamed == "_ENV" {
        return None;
    }
    tree.name_id(&renamed).is_none().then(|| {
        fixer.replace(
            format!("prefix `{name}` with `_`"),
            symbol.definition_span,
            renamed.into_bytes(),
        )
    })
}
