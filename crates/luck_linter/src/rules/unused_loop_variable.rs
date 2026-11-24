use luck_semantic::SymbolKind;

use crate::context::LintContext;
use crate::rule::{Rule, declare_lint_rule};
use crate::underscore_fix::unused_binding;

/// Flags a `for` loop variable that's never read (Luacheck 213).
///
/// It's separate from `unused_variable`, which skips loop variables, so a
/// project can allow loops that only count iterations and still catch unused
/// locals.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnusedLoopVariable;

declare_lint_rule!(
    UnusedLoopVariable,
    Style,
    Warning,
    "for-loop variable is declared but never read in the loop body",
    fix = Suggestion
);

impl Rule for UnusedLoopVariable {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let tree = context.scoping();

        for symbol in tree.symbols() {
            if !matches!(symbol.kind, SymbolKind::IteratorVariable | SymbolKind::NumericForVariable)
            {
                continue;
            }
            let name = tree.name(symbol.name);
            if name.starts_with('_') {
                continue;
            }

            let has_read = tree
                .resolved_references(symbol.id)
                .iter()
                .any(|&reference_id| tree.reference(reference_id).kind.is_read());
            if has_read {
                continue;
            }

            context.diagnostic(unused_binding(
                context,
                format!("unused loop variable `{name}`"),
                symbol,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &["for _ = 1, 10 do print() end", "for i = 1, 10 do print(i) end"];

    const FAIL: &[&str] = &[
        "for i = 1, 10 do print() end",
        "for k, v in pairs(t) do print(v) end",
        "for k, v in pairs(t) do print() end",
        // The rename shouldn't make `print(_k)` read the loop variable.
        "\
local _k = 1
for k, v in pairs({}) do print(_k, v) end",
        // Renaming the loop variable makes `i = 5` global.
        "for i = 1, 3 do i = 5 end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnusedLoopVariable, PASS, FAIL).test_and_snapshot();
    }
}
