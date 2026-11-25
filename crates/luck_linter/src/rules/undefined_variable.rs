use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UndefinedVariable;

declare_lint_rule!(UndefinedVariable, Correctness, Error, "use of undefined variable");

impl Rule for UndefinedVariable {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let semantic = context.semantic;
        let tree = semantic.scoping();
        for reference in tree.global_references() {
            let name = tree.name(reference.name);
            // `setting_global` owns writes, and a global this file writes
            // (`function f() end`, `counter = 0`) counts as defined
            // everywhere in it, or script-style code would error on every use.
            if reference.kind.is_write()
                || semantic.is_known_global_name(reference.name)
                || name == "_"
                || tree.is_assigned_global(reference.name)
            {
                continue;
            }
            context.diagnostic(
                Finding::new(format!("undefined variable `{name}`"), reference.span).with_help(
                    "declare it with `local`, or add it to `extra_globals` or a `std` definition",
                ),
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
print(counter)
counter = 0
function helper() end
helper()",
        "\
fresh = 1
print(_)",
        "counter += 1",
        // Type functions have the `types` library.
        "\
type function t1(a)
 return types.string
end",
        "print(tostring(1))",
    ];

    const FAIL: &[&str] = &[
        "print(missing)",
        // A vararg function has no implicit `arg`.
        "local function f(...) return arg end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UndefinedVariable, PASS, FAIL).test_and_snapshot();
    }
}
