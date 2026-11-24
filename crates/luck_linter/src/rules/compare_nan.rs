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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
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
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::CompareNan, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_equality_with_nan() {
        let diags = run("local same = x == 0/0");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_inequality_with_nan_on_left() {
        let diags = run("local differs = 0/0 ~= x");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_every_spelling_of_zero() {
        for nan in ["0.0/0", "0/0.0", "0x0/0", "0e0/0"] {
            let diags = run(&format!("local same = x == {nan}"));
            assert_eq!(diags.len(), 1, "{nan}: {diags:?}");
        }
    }

    #[test]
    fn flags_any_constant_that_evaluates_to_nan() {
        for nan in ["-(0/0)", "1e999 - 1e999", "0/0 + 1"] {
            let diags = run(&format!("local same = x == {nan}"));
            assert_eq!(diags.len(), 1, "{nan}: {diags:?}");
        }
    }

    #[test]
    fn ignores_division_that_isnt_nan() {
        let diags = run("local same = x == 0/1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_self_comparison_nan_check() {
        let diags = run("local is_nan = x ~= x");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_parenthesized_nan() {
        let diags = run("local same = x == (0/0)");
        assert_eq!(diags.len(), 1, "parentheses don't change the NaN: {diags:?}");
    }
}
