use luck_ast::{AstKind, AstType, Expression};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::BinOp;

use crate::binary::operator_pairs;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::number::number_value;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CompareNan;

declare_lint_rule!(
    CompareNan,
    Correctness,
    Error,
    "comparison with NaN (`0/0`) always has the same result"
);

impl Rule for CompareNan {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::BinaryOp]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::BinaryOp(run) = node.kind() else {
            return;
        };
        if !run.links.first().op.is_comparison() {
            return;
        }
        let is_nan = |side: Option<&Expression<'_>>| {
            side.and_then(|operand| number_value(operand, context)).is_some_and(f64::is_nan)
        };
        for pair in operator_pairs(run) {
            if matches!(pair.op, BinOp::Eq | BinOp::Ne) && (is_nan(pair.lhs) || is_nan(pair.rhs)) {
                context.diagnostic(
                    Finding::new(
                        "NaN (`0/0`) equals nothing, so this comparison always has the same result",
                        pair.span(context),
                    )
                    .with_help("use `x ~= x` to check for NaN"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &["local same = x == 0/1", "local is_nan = x ~= x"];

    const FAIL: &[&str] = &[
        "local same = x == 0/0",
        "local differs = 0/0 ~= x",
        "local same = x == 0.0/0",
        "local same = x == 0/0.0",
        "local same = x == 0x0/0",
        "local same = x == 0e0/0",
        "local same = x == -(0/0)",
        "local same = x == 1e999 - 1e999",
        "local same = x == 0/0 + 1",
        "local same = x == (0/0)",
        "\
local x = 1
if x == 0/0 then end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::CompareNan, PASS, FAIL).test_and_snapshot();
    }
}
