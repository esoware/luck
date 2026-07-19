use std::collections::HashMap;

use luck_semantic::scope::{Reference, ReferenceKind, ScopeId, ScopeKind};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule};

pub struct GlobalUsedAsLocal;

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
        "Global variable is only used inside one function; use a local."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let tree = &ctx.semantic.scope_tree;
        let mut by_name: HashMap<&str, Vec<&Reference>> = HashMap::new();
        for reference in tree.unresolved_references() {
            by_name
                .entry(reference.name.as_str())
                .or_default()
                .push(reference);
        }

        let mut diagnostics = Vec::new();
        for (name, references) in by_name {
            if name == "_" || ctx.semantic.is_known_global(name) {
                continue;
            }
            let is_assigned = references.iter().any(|reference| {
                matches!(
                    reference.kind,
                    ReferenceKind::Write | ReferenceKind::ReadWrite
                )
            });
            if !is_assigned {
                continue;
            }
            let mut common_function: Option<ScopeId> = None;
            let mut all_in_one_function = true;
            for reference in &references {
                let Some(function_scope) = enclosing_function(ctx, reference.scope) else {
                    all_in_one_function = false;
                    break;
                };
                match common_function {
                    None => common_function = Some(function_scope),
                    Some(seen) if seen != function_scope => {
                        all_in_one_function = false;
                        break;
                    }
                    Some(_) => {}
                }
            }
            if !all_in_one_function {
                continue;
            }
            let first = references
                .iter()
                .min_by_key(|reference| reference.span.start)
                .expect("group is non-empty");
            diagnostics.push(
                LintDiagnostic::new(
                    "global_used_as_local",
                    format!("global '{name}' is only used in one enclosing function"),
                    first.span,
                )
                .with_help("declare it `local` inside that function".to_string()),
            );
        }
        diagnostics.sort_by_key(|diag| diag.span.start);
        diagnostics
    }
}

/// The innermost `Function`-kind scope containing `scope`, or `None`
/// for module-level references.
fn enclosing_function(ctx: &LintContext, scope: ScopeId) -> Option<ScopeId> {
    let tree = &ctx.semantic.scope_tree;
    let mut current = Some(scope);
    while let Some(scope_id) = current {
        let scope = &tree.scopes[scope_id.index()];
        if scope.kind == ScopeKind::Function {
            return Some(scope_id);
        }
        current = scope.parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::GlobalUsedAsLocal;
    use crate::diagnostic::LintDiagnostic;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&GlobalUsedAsLocal, source, LuaVersion::Lua54)
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
        // Top-level globals are the module's exports; not local material.
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
        // The inner function is a different function scope, so the
        // global escapes the outer one.
        let diags = run(
            "local function f()\n    state = 1\n    local function g()\n        return state\n    end\n    return g\nend\nf()",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
