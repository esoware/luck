use luck_diagnostics::Tag;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Semantic analysis resolves each `goto` to the label it jumps to, as
/// `lparser.c` does, so a label is used exactly when one reaches it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnusedLabel;

declare_lint_rule!(
    UnusedLabel,
    Style,
    Warning,
    "label declared but never targeted by a `goto` in the same function"
);

impl Rule for UnusedLabel {
    fn run_once(&self, context: &LintContext<'_>) {
        let tree = context.scoping();
        for label in tree.labels().iter().filter(|label| !label.is_used) {
            context.diagnostic(
                Finding::new(
                    format!("label `{}` is never targeted by a `goto`", tree.name(label.name)),
                    label.span,
                )
                .with_tag(Tag::Unnecessary)
                .with_help("remove the label or add a `goto` that targets it"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "do ::a:: print(1) goto a end",
        "do ::a:: do print() goto a end end",
        "::start:: print(1) goto start",
    ];

    const FAIL: &[&str] = &[
        "do ::a:: print(1) end",
        // `g`'s `goto` takes its own label, since a `goto` can't leave its
        // function, and no `goto` takes `f`'s.
        "function f() ::a:: print() end function g() ::a:: goto a end",
        "::start:: print(1)",
        // No label is visible across functions, so the inner `goto` takes the inner
        // label and the outer one stays unused.
        "::a:: local f = function() ::a:: goto a end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnusedLabel, PASS, FAIL).test_and_snapshot();
    }
}
