use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SettingGlobal;

declare_lint_rule!(
    SettingGlobal,
    Correctness,
    Warning,
    "implicit global variable assignment (missing `local`?)"
);

impl Rule for SettingGlobal {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let semantic = context.semantic;
        let tree = semantic.scoping();

        for reference in tree.global_references() {
            if !reference.kind.is_write() {
                continue;
            }
            // Writing a known global like `print` isn't a forgotten `local`.
            if semantic.is_known_global_name(reference.name) {
                continue;
            }
            let name = tree.name(reference.name);
            // `_` is a conventional discard name, not an accidental global.
            if name == "_" {
                continue;
            }

            context.diagnostic(
                Finding::new(
                    format!("setting global variable `{name}` (missing `local`?)"),
                    reference.span,
                )
                .with_help("add `local` before the first assignment"),
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
print = print
_ = 1",
        "\
local counter
counter = 1",
    ];

    const FAIL: &[&str] = &["counter = 1"];

    #[test]
    fn test() {
        Tester::new(RuleId::SettingGlobal, PASS, FAIL).test_and_snapshot();
    }
}
