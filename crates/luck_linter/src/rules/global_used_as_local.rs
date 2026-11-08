use luck_semantic::{Reference, ScopeId, ScopeKind, ScopeTree};

use crate::diagnostic::{Category, Finding, Severity};
use crate::rule::{LintContext, Rule, WholeRule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct GlobalUsedAsLocal;

impl Rule for GlobalUsedAsLocal {
    fn name(&self) -> &'static str {
        "global_used_as_local"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "global variable is only used inside one function; use a local"
    }
}

impl WholeRule for GlobalUsedAsLocal {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let tree = &context.semantic.scope_tree;
        let mut unresolved_references: Vec<&Reference> = tree.unresolved_references().collect();
        unresolved_references.sort_by(|left, right| left.name.cmp(&right.name));

        let mut findings = Vec::new();
        for references in unresolved_references.chunk_by(|left, right| left.name == right.name) {
            let Some(first) = references.iter().min_by_key(|reference| reference.span.start) else {
                continue;
            };
            let name = first.name.as_str();
            if name == "_" || context.semantic.is_known_global(name) {
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

/// The innermost `Function`-kind scope containing `scope`, or `None`
/// for module-level references.
fn enclosing_function(tree: &ScopeTree, scope: ScopeId) -> Option<ScopeId> {
    let mut current = Some(scope);
    while let Some(scope_id) = current {
        let current_scope = tree.scope(scope_id);
        if current_scope.kind == ScopeKind::Function {
            return Some(scope_id);
        }
        current = current_scope.parent;
    }
    None
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
        let diags = run(
            "local function f()\n    counter = 0\n    counter = counter + 1\n    return counter\nend\nf()",
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("counter"), "{diags:?}");
    }

    #[test]
    fn flags_write_only_global_in_one_function() {
        let diags = run("local function f()\n    cache = {}\nend\nf()");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_global_used_in_two_functions() {
        let diags = run(
            "local function f()\n    shared_state = 1\nend\nlocal function g()\n    return shared_state\nend\nf()\ng()",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_global_also_used_at_top_level() {
        let diags = run("local function f()\n    counter = 1\nend\nf()\nprint(counter)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_module_level_global() {
        // Top-level globals may be the module's exports.
        let diags = run("counter = 0\ncounter = counter + 1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_read_only_global() {
        let diags = run("local function f()\n    return config\nend\nf()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_stdlib_globals() {
        let diags = run("local function f()\n    print(1)\nend\nf()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_nested_function_split_usage() {
        // `g` is a separate function, so `state` is used in two.
        let diags = run(
            "local function f()\n    state = 1\n    local function g()\n        return state\n    end\n    return g\nend\nf()",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
