use luck_ast::{AstKind, AstType};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::number::number_value;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ReversedForLoop;

declare_lint_rule!(
    ReversedForLoop,
    Suspicious,
    Warning,
    "numeric `for` loop counts down without a negative step"
);

impl Rule for ReversedForLoop {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::NumericFor]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        if let AstKind::NumericFor(numeric_for) = node.kind()
            && numeric_for.step.is_none()
            && let Some(start) = number_value(&numeric_for.start, context)
            && let Some(limit) = number_value(&numeric_for.limit, context)
            && start > limit
        {
            context.diagnostic(
                Finding::new(
                    format!(
                        "`for` loop from {start} to {limit} without a negative step never runs"
                    ),
                    numeric_for.span,
                )
                .with_help("add a negative step: `for i = start, limit, -1 do`"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "for i = 10, 1, -1 do print(i) end",
        "for i = 1, 10 do print(i) end",
        "\
local n = 0
for i = 10, n do print(i) end",
        // The limit reads as 1.8e19.
        "for i = 1, 0xFFFFFFFFFFFFFFFF do print(i) end",
    ];

    const FAIL: &[&str] = &[
        "for i = 10, 1 do print(i) end",
        "for i = 0x10, 1 do print(i) end",
        "for i = -1, -5 do print(i) end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::ReversedForLoop, PASS, FAIL).test_and_snapshot();
    }
}
