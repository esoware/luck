use luck_semantic::SymbolKind;

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};
use crate::underscore_fix::UnderscoreFixes;

/// Flags a `for` loop variable that's never read (Luacheck 213).
///
/// It's separate from `unused_variable`, which skips loop variables, so a
/// project can allow loops that only count iterations and still catch unused
/// locals.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UnusedLoopVariable;

impl_rule!(
    UnusedLoopVariable,
    "unused_loop_variable",
    Style,
    Warning,
    "for-loop variable is declared but never read in the loop body"
);

impl WholeRule for UnusedLoopVariable {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let semantic = context.semantic;
        let fixes = UnderscoreFixes::new(semantic);
        let mut findings = Vec::new();

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

            findings.push(
                Finding::new(
                    format!("unused loop variable `{}`", symbol.name),
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

    #[test]
    fn flags_without_fix_that_captures_existing_underscore_name() {
        let source = "\
local _k = 1
for k, v in pairs({}) do print(_k, v) end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_ne!(
            apply_one_pass(source, &diags),
            "\
local _k = 1
for _k, v in pairs({}) do print(_k, v) end",
            "the rename shouldn't make `print(_k)` read the loop variable"
        );
    }

    #[test]
    fn flags_without_fix_when_loop_variable_is_written() {
        let diags = run("for i = 1, 3 do i = 5 end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].fix.is_none(),
            "renaming the loop variable makes `i = 5` global: {diags:?}"
        );
    }
}
