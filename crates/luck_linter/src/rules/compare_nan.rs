use luck_ast::{Expression, unwrap_parentheses};
use luck_token::{BinOp, LuaVersion};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::number::is_number_literal;
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct CompareNan;

impl_rule!(
    CompareNan,
    "compare_nan",
    Correctness,
    Error,
    "comparison with NaN (`0/0`) always has the same result"
);

impl NodeRule for CompareNan {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::BinaryOp])
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let version = context.semantic.version;
        if let Expression::BinaryOp(binary) = expr
            && matches!(binary.op, BinOp::Eq | BinOp::Ne)
            && (is_nan_expr(&binary.left, version) || is_nan_expr(&binary.right, version))
        {
            findings.push(
                Finding::new(
                    "NaN (`0/0`) equals nothing, so this comparison always has the same result",
                    binary.span,
                )
                .with_help("use `x ~= x` to check for NaN"),
            );
        }
    }
}

/// Whether `expr` divides a zero literal by a zero literal, each in any
/// spelling, parentheses aside.
fn is_nan_expr(expr: &Expression, version: LuaVersion) -> bool {
    let Expression::BinaryOp(binary) = unwrap_parentheses(expr) else {
        return false;
    };
    binary.op == BinOp::Div
        && is_number_literal(&binary.left, 0.0, version)
        && is_number_literal(&binary.right, 0.0, version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&CompareNan, source, LuaVersion::Lua54)
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
