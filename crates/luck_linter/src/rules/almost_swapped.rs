use luck_ast::{AstKind, AstType, Expression, Statement, Var};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AlmostSwapped;

declare_lint_rule!(
    AlmostSwapped,
    Correctness,
    Error,
    "looks like a failed variable swap; use `a, b = b, a`"
);

impl Rule for AlmostSwapped {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        for (first, second) in block.stmts.iter().zip(block.stmts.iter().skip(1)) {
            if let Some(finding) = failed_swap(first, second) {
                context.diagnostic(finding);
            }
        }
    }
}

fn failed_swap(first_stmt: &Statement<'_>, second_stmt: &Statement<'_>) -> Option<Finding> {
    let (Statement::Assignment(first), Statement::Assignment(second)) = (first_stmt, second_stmt)
    else {
        return None;
    };
    let (
        [Var::Name(target_a)],
        [Expression::Name(value_a)],
        [Var::Name(target_b)],
        [Expression::Name(value_b)],
    ) = (&*first.targets, &*first.values, &*second.targets, &*second.values)
    else {
        return None;
    };
    // After `a = b`, `b = a` copies `b` back and the old `a` is lost.
    (target_a.text == value_b.text && target_b.text == value_a.text).then(|| {
        Finding::new(
            format!(
                "`{} = {}; {} = {}` doesn't swap",
                target_a.text, value_a.text, target_b.text, value_b.text
            ),
            first.span.merge(second.span),
        )
        .with_help(format!(
            "use `{0}, {1} = {1}, {0}` to swap both at once",
            target_a.text, target_b.text
        ))
    })
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "\
local a, b = 1, 2
a, b = b, a",
        "\
local a, b = 1, 2
a = b
b = 3",
    ];

    const FAIL: &[&str] = &[
        "\
local a, b = 1, 2
a = b
b = a",
        "\
local f = function() local a, b = 1, 2
a = b
b = a end",
        // A `;` doesn't change the failed swap.
        "\
local a, b = 1, 2
a = b; b = a",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::AlmostSwapped, PASS, FAIL).test_and_snapshot();
    }
}
