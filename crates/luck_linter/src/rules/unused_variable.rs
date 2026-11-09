use luck_semantic::SymbolKind;

use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit};
use crate::rule::{LintContext, WholeRule, impl_rule};

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
        let mut findings = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.name.starts_with('_') {
                continue;
            }

            // Luau: an exported binding is read through the module's export
            // table, and renaming it would change the module's API.
            if symbol.is_exported {
                continue;
            }

            // `unused_argument` and `unused_loop_variable` cover parameters
            // and loop variables.
            let kind_name = match symbol.kind {
                SymbolKind::Local => "variable",
                SymbolKind::FunctionName => "function",
                SymbolKind::Parameter
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

            // Renaming only the declaration would turn any later write into a
            // global write, so only a symbol with no references gets the fix.
            let fix = symbol.reference_ids.is_empty().then(|| Fix {
                description: format!("prefix `{}` with `_`", symbol.name),
                edits: vec![TextEdit {
                    span: symbol.definition_span,
                    replacement: format!("_{}", symbol.name),
                }],
            });

            findings.push(
                Finding::new(
                    format!("unused {kind_name} `{}`", symbol.name),
                    symbol.definition_span,
                )
                .with_help("prefix with `_` to suppress this warning")
                .with_optional_fix(fix),
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
}
