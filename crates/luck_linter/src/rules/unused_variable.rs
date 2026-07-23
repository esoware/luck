use luck_semantic::scope::{ReferenceKind, SymbolKind};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct UnusedVariable;

impl Rule for UnusedVariable {
    fn name(&self) -> &'static str {
        "unused_variable"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "variable is declared but never read"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let semantic = ctx.semantic;
        let mut diagnostics = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if symbol.name == "_" || symbol.name.starts_with('_') {
                continue;
            }

            // Luau: an exported binding's identifier is the export
            // table's key, so it is read externally and renaming it
            // would change the module's public API.
            if symbol.is_exported {
                continue;
            }

            // Parameters and loop variables are owned by unused_argument and
            // unused_loop_variable; reporting them here would double-fire.
            let kind_str = match symbol.kind {
                SymbolKind::Local => "variable",
                SymbolKind::FunctionName => "function",
                SymbolKind::Parameter
                | SymbolKind::IteratorVariable
                | SymbolKind::NumericForVariable => continue,
            };

            let has_read = symbol.reference_ids.iter().any(|&ref_id| {
                matches!(
                    semantic.scope_tree.reference(ref_id).kind,
                    ReferenceKind::Read | ReferenceKind::ReadWrite
                )
            });

            if has_read {
                continue;
            }

            // Write-only symbols get a diagnostic but no fix: renaming only
            // the declaration turns every later write into a global write.
            // The `_` rename is safe only when nothing references it at all.
            let fix = if symbol.reference_ids.is_empty() {
                Some(Fix {
                    description: format!("prefix `{}` with `_`", symbol.name),
                    edits: vec![TextEdit {
                        span: symbol.definition_span,
                        replacement: format!("_{}", symbol.name),
                    }],
                })
            } else {
                None
            };

            diagnostics.push(
                LintDiagnostic::new(
                    "unused_variable",
                    format!("unused {kind_str} `{}`", symbol.name),
                    symbol.definition_span,
                )
                .with_help("prefix with `_` to suppress this warning".to_string())
                .with_fix_opt(fix),
            );
        }

        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&UnusedVariable, source, LuaVersion::Luau)
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
