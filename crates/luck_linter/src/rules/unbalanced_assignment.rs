use luck_ast::{AstKind, AstType, Expression};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnbalancedAssignment;

declare_lint_rule!(
    UnbalancedAssignment,
    Suspicious,
    Warning,
    "assignment has a different number of targets and values"
);

impl Rule for UnbalancedAssignment {
    const NODE_TYPES: AstTypesBitset =
        AstTypesBitset::from_types(&[AstType::LocalAssignment, AstType::Assignment]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let kind = node.kind();
        if let AstKind::LocalAssignment(local) = kind {
            let name_count = local.names.len();
            let value_count = local.exprs.len();
            if !local.exprs.last().is_some_and(Expression::is_multi_value)
                && name_count != value_count
                && value_count > 0
            {
                context.diagnostic(Finding::new(
                    format!("{name_count} name(s) but {value_count} value(s) in assignment"),
                    local.span,
                ));
            }
        } else if let AstKind::Assignment(assignment) = kind {
            let target_count = assignment.targets.len();
            let value_count = assignment.values.len();
            if !assignment.values.last().is_some_and(Expression::is_multi_value)
                && target_count != value_count
            {
                context.diagnostic(Finding::new(
                    format!("{target_count} target(s) but {value_count} value(s) in assignment"),
                    assignment.span,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local a, b, c = 1, f()
local function g(...) a, b, c = ... end",
        "local a, b",
        "\
local a, b = 1, 2
a, b = b, a",
    ];

    const FAIL: &[&str] = &[
        "local a, b, c = 1, 2",
        "\
local a, b
a, b = 1, 2, 3",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnbalancedAssignment, PASS, FAIL).test_and_snapshot();
    }
}
