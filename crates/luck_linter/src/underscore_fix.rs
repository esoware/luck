use luck_semantic::{SemanticAnalysis, Symbol};

use crate::fix::{Fix, TextEdit};

/// Builds `_name` fixes for one chunk, and withholds one that could change
/// what a name resolves to.
pub(crate) struct UnderscoreFixes<'ast> {
    semantic: &'ast SemanticAnalysis<'ast>,
}

impl<'ast> UnderscoreFixes<'ast> {
    pub(crate) const fn new(semantic: &'ast SemanticAnalysis<'ast>) -> Self {
        Self { semantic }
    }

    /// A fix prefixing `symbol` with `_`, or `None` when a reference would
    /// stay behind under the old name, or the new name is already taken.
    ///
    /// The chunk's names are every name it spells, declared or referenced,
    /// so a rename to any of them is refused, wherever it sits.
    pub(crate) fn fix_for(&self, symbol: &Symbol) -> Option<Fix> {
        let tree = self.semantic.scope_tree();
        if !tree.resolved_references(symbol.id).is_empty() {
            return None;
        }
        let name = tree.name(symbol.name);
        let renamed = format!("_{name}");
        // Lua 5.2+: a binding named `_ENV` redirects every global in scope.
        if self.semantic.version().has_env_upvalue() && renamed == "_ENV" {
            return None;
        }
        tree.name_id(&renamed).is_none().then(|| Fix {
            description: format!("prefix `{name}` with `_`"),
            edits: vec![TextEdit { span: symbol.definition_span, replacement: renamed }],
        })
    }
}
