//! The fix the unused-binding rules share: renaming `name` to `_name`.

use core::cell::OnceCell;

use luck_semantic::{SemanticAnalysis, Symbol};
use rustc_hash::FxHashSet;

use crate::fix::{Fix, TextEdit};

/// Builds `_name` fixes for one chunk, and withholds one that could change
/// what a name resolves to.
pub(crate) struct UnderscoreFixes<'semantic> {
    semantic: &'semantic SemanticAnalysis,
    /// Every name the chunk spells, gathered when the first fix is wanted.
    /// A rename to any of them is refused, wherever it sits.
    spelled_names: OnceCell<FxHashSet<&'semantic str>>,
}

impl<'semantic> UnderscoreFixes<'semantic> {
    pub(crate) const fn new(semantic: &'semantic SemanticAnalysis) -> Self {
        Self { semantic, spelled_names: OnceCell::new() }
    }

    /// A fix prefixing `symbol` with `_`, or `None` when a reference would
    /// stay behind under the old name, or the new name is already taken.
    pub(crate) fn fix_for(&self, symbol: &Symbol) -> Option<Fix> {
        if !symbol.reference_ids.is_empty() {
            return None;
        }
        let name = symbol.name.as_str();
        let renamed = format!("_{name}");
        // Lua 5.2+: a binding named `_ENV` redirects every global in scope.
        if self.semantic.version.has_env_upvalue() && renamed == "_ENV" {
            return None;
        }
        let spelled_names = self.spelled_names.get_or_init(|| {
            let tree = &self.semantic.scope_tree;
            let declared = tree.symbols.iter().map(|declared| declared.name.as_str());
            declared.chain(tree.references.iter().map(|used| used.name.as_str())).collect()
        });
        (!spelled_names.contains(renamed.as_str())).then(|| Fix {
            description: format!("prefix `{name}` with `_`"),
            edits: vec![TextEdit { span: symbol.definition_span, replacement: renamed }],
        })
    }
}
