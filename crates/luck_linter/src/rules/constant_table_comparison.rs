use luck_ast::{AstKind, AstType, Expression};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::BinOp;

use crate::binary::operator_pairs;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ConstantTableComparison;

declare_lint_rule!(
    ConstantTableComparison,
    Correctness,
    Error,
    "comparison with a table literal, which compares by identity"
);

impl Rule for ConstantTableComparison {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::BinaryOp]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::BinaryOp(run) = node.kind() else {
            return;
        };
        if !run.links.first().op.is_comparison() {
            return;
        }
        let is_table = |operand: Option<&Expression<'_>>| {
            matches!(operand, Some(Expression::TableConstructor(_)))
        };
        for pair in operator_pairs(run) {
            if matches!(pair.op, BinOp::Eq | BinOp::Ne)
                && (is_table(pair.lhs) || is_table(pair.rhs))
            {
                context.diagnostic(
                    Finding::new(
                        "tables compare by identity, so a table literal never equals another value",
                        pair.span(context),
                    )
                    .with_help("compare table contents instead"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &["\
local t = {}
local same = x == t"];

    const FAIL: &[&str] = &[
        "local same = x == {}",
        "local differs = {1} ~= x",
        "\
local x = {}
if x == {} then end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::ConstantTableComparison, PASS, FAIL).test_and_snapshot();
    }
}
