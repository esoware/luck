use luck_semantic::SymbolKind;

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};
use crate::underscore_fix::UnderscoreFixes;

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnusedVariable;

impl_rule!(
    UnusedVariable,
    "unused_variable",
    Correctness,
    Warning,
    "variable is declared but never read"
);

impl WholeRule for UnusedVariable {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let fixes = UnderscoreFixes::new(semantic);
        let mut findings = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.name.starts_with('_') {
                continue;
            }

            // Luau: an exported binding is read through the module's export
            // table, and renaming it would change the module's API. A
            // `<close>` local is used by its scope exit.
            if symbol.is_exported || symbol.is_close {
                continue;
            }

            // `unused_argument` and `unused_loop_variable` cover parameters
            // and loop variables.
            let kind_name = match symbol.kind {
                SymbolKind::Local => "variable",
                SymbolKind::FunctionName => "function",
                SymbolKind::Parameter
                | SymbolKind::ImplicitSelf
                | SymbolKind::IteratorVariable
                | SymbolKind::NumericForVariable => continue,
            };

            let has_read = symbol
                .reference_ids
                .iter()
                .any(|&reference_id| semantic.scope_tree.reference(reference_id).kind.is_read());

            if has_read {
                continue;
            }

            findings.push(
                Finding::new(
                    format!("unused {kind_name} `{}`", symbol.name),
                    symbol.definition_span,
                )
                .with_help("prefix with `_` to suppress this warning")
                .with_optional_fix(fixes.fix_for(symbol)),
            );
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fix::apply_one_pass;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&UnusedVariable, source, LuaVersion::Luau)
    }

    #[test]
    fn ignores_exported_bindings() {
        let diags = run(
            "export local answer = 42\nexport const LIMIT = 1i\nexport function helper()\nreturn 1\nend",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_plain_local_next_to_exports() {
        let diags = run("export local answer = 42\nlocal dead = 1");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`dead`"));
    }

    #[test]
    fn ignores_variable_read_by_typeof() {
        // typeof(expr) inside a type position references the runtime
        // binding; it must count as a use.
        let diags = run("local n = 1\ntype T = typeof(n)\nexport type U = T");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_variable_read_by_typeof_in_cast() {
        let diags = run("local n = 1\nlocal m = (nil :: typeof(n))\nprint(m)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_variable_unused_despite_type_alias() {
        let diags = run("local n = 1\ntype T = number\nexport type U = T");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_without_fix_that_captures_existing_underscore_name() {
        let source = "local _x = 1\nlocal x = 2\nprint(_x)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_ne!(
            apply_one_pass(source, &diags),
            "local _x = 1\nlocal _x = 2\nprint(_x)",
            "the rename shouldn't make `print(_x)` read the second local"
        );
    }

    #[test]
    fn ignores_close_variable() {
        let source = "local lock <close> = acquire()\nprint(1)";
        let diags = run_rule(&UnusedVariable, source, LuaVersion::Lua54);
        assert!(diags.is_empty(), "a `<close>` local is used by its scope exit: {diags:?}");
    }

    #[test]
    fn flags_without_fix_that_would_bind_env() {
        let diags = run_rule(&UnusedVariable, "local ENV = 1", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "a local `_ENV` redirects every global: {diags:?}");
    }

    #[test]
    fn flags_with_fix_when_prefixed_name_is_free() {
        let diags = run("local x = 2\nprint(_y)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_some(), "{diags:?}");
    }
}
