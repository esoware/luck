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
    let ([first_target], [first_value], [second_target], [second_value]) =
        (&*first.targets, &*first.values, &*second.targets, &*second.values)
    else {
        return None;
    };
    let target_a = var_name(first_target)?;
    let value_a = expr_var_name(first_value)?;
    let target_b = var_name(second_target)?;
    let value_b = expr_var_name(second_value)?;
    // After `a = b`, `b = a` copies `b` back and the old `a` is lost.
    (target_a == value_b && target_b == value_a).then(|| {
        Finding::new(
            format!("`{target_a} = {value_a}; {target_b} = {value_b}` doesn't swap"),
            first.span.merge(second.span),
        )
        .with_help(format!(
            "use `{target_a}, {target_b} = {target_b}, {target_a}` to swap both at once"
        ))
    })
}

fn var_name<'ast>(var: &Var<'ast>) -> Option<&'ast str> {
    let Var::Name(name) = var else {
        return None;
    };
    Some(name.text.as_str())
}

fn expr_var_name<'ast>(expr: &Expression<'ast>) -> Option<&'ast str> {
    let Expression::Name(name) = expr else {
        return None;
    };
    Some(name.text.as_str())
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
