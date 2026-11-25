use luck_semantic::SymbolKind;

use crate::context::LintContext;
use crate::rule::{Rule, declare_lint_rule};
use crate::underscore_fix::unused_binding;

/// The `_` prefix is a suggestion, because it silences the warning rather
/// than answering it, and an unused variable is often a misspelled use.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnusedVariable;

declare_lint_rule!(
    UnusedVariable,
    Correctness,
    Warning,
    "variable is declared but never read",
    fix = Suggestion
);

impl Rule for UnusedVariable {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let tree = context.scoping();

        for symbol in tree.symbols() {
            let name = tree.name(symbol.name);
            if name.starts_with('_') {
                continue;
            }

            // An exported binding is read through the module's export table,
            // and renaming it would change the module's API.
            if symbol.is_exported {
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

            let has_read = tree
                .resolved_references(symbol.id)
                .iter()
                .any(|&reference_id| tree.reference(reference_id).kind.is_read());

            if has_read {
                continue;
            }

            context.diagnostic(unused_binding(
                context,
                format!("unused {kind_name} `{name}`"),
                symbol,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
export local answer = 42
export const LIMIT = 1i
export function helper()
return 1
end",
        // `typeof(expr)` in a type reads the runtime binding, so it counts as a
        // use.
        "\
local n = 1
type T = typeof(n)
export type U = T",
        "\
local n = 1
local m = (nil :: typeof(n))
print(m)",
        "local _unused = 1",
    ];

    const FAIL: &[&str] = &[
        "\
export local answer = 42
local dead = 1",
        "\
local n = 1
type T = number
export type U = T",
        // The rename shouldn't make `print(_x)` read the second local.
        "\
local _x = 1
local x = 2
print(_x)",
        "\
local x = 2
print(_y)",
        "local unused = 1",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnusedVariable, PASS, FAIL).test_and_snapshot();
    }
}
