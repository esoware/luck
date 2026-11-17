use luck_semantic::{Reference, ScopeId, ScopeKind, ScopeTree};

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct GlobalUsedAsLocal;

impl_rule!(
    GlobalUsedAsLocal,
    "global_used_as_local",
    Style,
    Warning,
    "global variable is only used inside one function; use a local"
);

impl WholeRule for GlobalUsedAsLocal {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let tree = context.semantic.scope_tree();
        let mut global_references: Vec<&Reference> = tree.global_references().collect();
        global_references.sort_by_key(|reference| reference.name);

        let mut findings = Vec::new();
        for references in global_references.chunk_by(|left, right| left.name == right.name) {
            let Some(first) = references.iter().min_by_key(|reference| reference.span.start) else {
                continue;
            };
            let name = tree.name(first.name);
            if name == "_" || context.semantic.is_known_global_name(first.name) {
                continue;
            }
            let is_assigned = references.iter().any(|reference| reference.kind.is_write());
            if !is_assigned {
                continue;
            }
            // Every reference sits inside a function, and all in the same one.
            let mut functions =
                references.iter().map(|reference| enclosing_function(tree, reference.scope));
            let Some(Some(function)) = functions.next() else {
                continue;
            };
            if !functions.all(|other| other == Some(function)) {
                continue;
            }
            findings.push(
                Finding::new(
                    format!("global `{name}` is only used inside one function"),
                    first.span,
                )
                .with_help("declare it `local` inside that function"),
            );
        }
        findings.sort_by_key(|finding| finding.span.start);
        findings
    }
}

fn enclosing_function(tree: &ScopeTree<'_>, scope: ScopeId) -> Option<ScopeId> {
    let function = tree.scope(scope).function;
    (tree.scope(function).kind == ScopeKind::Function).then_some(function)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&GlobalUsedAsLocal, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_global_confined_to_one_function() {
        let diags = run("\
local function f()
    counter = 0
    counter = counter + 1
    return counter
end
f()");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("counter"), "{diags:?}");
    }

    #[test]
    fn flags_write_only_global_in_one_function() {
        let diags = run("\
local function f()
    cache = {}
end
f()");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_global_used_in_two_functions() {
        let diags = run("\
local function f()
    shared_state = 1
end
local function g()
    return shared_state
end
f()
g()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_global_also_used_at_top_level() {
        let diags = run("\
local function f()
    counter = 1
end
f()
print(counter)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_module_level_global() {
        // Top-level globals may be the module's exports.
        let diags = run("\
counter = 0
counter = counter + 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_read_only_global() {
        let diags = run("\
local function f()
    return config
end
f()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_stdlib_globals() {
        let diags = run("\
local function f()
    print(1)
end
f()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_nested_function_split_usage() {
        // `g` is a separate function, so `state` is used in two.
        let diags = run("\
local function f()
    state = 1
    local function g()
        return state
    end
    return g
end
f()");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
