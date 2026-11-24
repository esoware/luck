use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::local_flow::is_field_write_prefix;
use crate::rule::{Rule, declare_lint_rule};

/// Flags a field or index write on a local declared without a value, before
/// any assignment can have reached it (Luacheck 341). `local x; x.foo = 1`
/// indexes `nil`.
///
/// The scope tree records the `x` in `x.foo = ...` as a plain read, and its
/// parents tell it apart from the `x` of `print(x)`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MutatingUninitialized;

declare_lint_rule!(
    MutatingUninitialized,
    Correctness,
    Warning,
    "field or index assignment on an uninitialized local"
);

impl Rule for MutatingUninitialized {
    fn run_once(&self, context: &LintContext<'_>) {
        let tree = context.scoping();
        for &(symbol_id, reference_id) in &context.local_flow().first_unassigned_uses {
            let first = tree.reference(reference_id);
            if !is_field_write_prefix(first, context) {
                continue;
            }
            context.diagnostic(
                Finding::new(
                    format!(
                        "field/index assignment on uninitialized `{}` (value is `nil`)",
                        tree.name(tree.symbol(symbol_id).name)
                    ),
                    first.span,
                )
                .with_help("initialize the local before mutating its fields"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local x = {}
x.foo = 1",
        "\
local x
x = {}
x.foo = 1",
        "\
local x
print(x)
x.foo = 1",
        "\
local _x
_x.foo = 1",
        // The closure runs after the assignment.
        "\
local M
local function init() M.x = 1 end
M = {}
init()",
    ];

    const FAIL: &[&str] = &[
        "\
local x
x.foo = 1",
        "\
local x
x[1] = 2",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::MutatingUninitialized, PASS, FAIL).test_and_snapshot();
    }
}
