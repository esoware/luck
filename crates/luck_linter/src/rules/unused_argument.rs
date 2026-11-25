use luck_semantic::SymbolKind;

use crate::context::LintContext;
use crate::rule::{Rule, declare_lint_rule};
use crate::underscore_fix::unused_binding;

/// Flags a function parameter that's never referenced (Luacheck 212).
///
/// Callbacks and methods often ignore parameters their interface requires,
/// so this has its own toggle, separate from `unused_variable`, which skips
/// parameters.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnusedArgument;

declare_lint_rule!(
    UnusedArgument,
    Style,
    Warning,
    "function parameter is declared but never referenced",
    fix = Suggestion
);

impl Rule for UnusedArgument {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let tree = context.scoping();

        for symbol in tree.symbols() {
            if symbol.kind != SymbolKind::Parameter {
                continue;
            }
            let name = tree.name(symbol.name);
            // A parameter spelled `self` is a receiver the interface requires.
            if name == "self"
                || name.starts_with('_')
                || !tree.resolved_references(symbol.id).is_empty()
            {
                continue;
            }

            context.diagnostic(unused_binding(
                context,
                format!("unused argument `{name}`"),
                symbol,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &["local function f(_x) end", "local function f(x) return x end"];

    const FAIL: &[&str] = &[
        "local function f(x) end",
        // Only `x` is flagged. `self` is the implicit method receiver.
        "\
local obj = {}
function obj:method(x) end",
        "local function f(a, b, c) return a + c end",
        // The rename shouldn't make `return _a` read the second parameter.
        "local function f(_a, a) return _a end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnusedArgument, PASS, FAIL).test_and_snapshot();
    }
}
