use luck_semantic::Symbol;
use luck_stdlib::SemanticAnalysis;

use crate::fix::{Fix, RuleFixer};

/// A fix prefixing `symbol` with `_`, or `None` when a reference would stay
/// behind under the old name, or the new name is already taken.
///
/// The chunk's names are every name it spells, declared or referenced, so a
/// rename to any of them is refused, wherever it sits.
pub(crate) fn prefix_underscore(
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
