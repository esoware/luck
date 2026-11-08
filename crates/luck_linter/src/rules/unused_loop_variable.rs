use luck_semantic::SymbolKind;

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::fix::{Fix, TextEdit};
use crate::rule::{LintContext, Rule, WholeRule};

/// Flags a `for` loop variable that's never read (Luacheck 213).
///
/// It's separate from `unused_variable`, which skips loop variables, so a
/// project can allow loops that only count iterations and still catch unused
/// locals.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnusedLoopVariable;

impl Rule for UnusedLoopVariable {
    fn name(&self) -> &'static str {
        "unused_loop_variable"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "for-loop variable is declared but never read in the loop body"
    }
}

impl WholeRule for UnusedLoopVariable {
    fn check(&self, context: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let semantic = context.semantic;
        let mut diags = Vec::new();

        for symbol in &semantic.scope_tree.symbols {
            if !matches!(symbol.kind, SymbolKind::IteratorVariable | SymbolKind::NumericForVariable)
            {
                continue;
            }
            if symbol.name.starts_with('_') {
                continue;
            }

            let has_read = symbol
                .reference_ids
                .iter()
                .any(|&reference_id| semantic.scope_tree.reference(reference_id).kind.is_read());
            if has_read {
                continue;
            }

            diags.push(
                LintDiagnostic::new(
                    "unused_loop_variable",
                    format!("unused loop variable `{}`", symbol.name),
                    symbol.definition_span,
                )
                .with_help("prefix with `_` to suppress this warning")
                .with_fix(Fix {
                    description: format!("rename `{}` to `_{}`", symbol.name, symbol.name),
                    edits: vec![TextEdit {
                        span: symbol.definition_span,
                        replacement: format!("_{}", symbol.name),
                    }],
                }),
            );
        }

        diags
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&UnusedLoopVariable, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_and_fixes_with_underscore_prefix() {
        let source = "for i = 1, 10 do print() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "for _i = 1, 10 do print() end");
    }

    #[test]
    fn flags_numeric_for_unused() {
        let diags = run("for i = 1, 10 do print() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`i`"));
    }

    #[test]
    fn ignores_underscore_numeric_for() {
        let diags = run("for _ = 1, 10 do print() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_generic_for_key_only() {
        let diags = run("for k, v in pairs(t) do print(v) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`k`"));
    }

    #[test]
    fn ignores_used_numeric_for() {
        let diags = run("for i = 1, 10 do print(i) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_both_unused_generic_for() {
        let diags = run("for k, v in pairs(t) do print() end");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }
}
