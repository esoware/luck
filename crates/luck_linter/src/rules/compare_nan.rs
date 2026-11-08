use luck_ast::Expression;
use luck_token::BinOp;

use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct CompareNan;

impl Rule for CompareNan {
    fn name(&self) -> &'static str {
        "compare_nan"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "comparison with NaN (`0/0`) always has the same result"
    }
}

impl NodeRule for CompareNan {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::BinaryOp])
    }
    fn on_expression(
        &self,
        expr: &Expression,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Expression::BinaryOp(binary) = expr
            && matches!(binary.op, BinOp::Eq | BinOp::Ne)
            && (is_nan_expr(&binary.left) || is_nan_expr(&binary.right))
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

/// Whether `expr` is spelled exactly `0/0`.
fn is_nan_expr(expr: &Expression) -> bool {
    let Expression::BinaryOp(binary) = expr else {
        return false;
    };
    binary.op == BinOp::Div && is_zero(&binary.left) && is_zero(&binary.right)
}

fn is_zero(expr: &Expression) -> bool {
    matches!(expr, Expression::Number(literal) if literal.text == "0")
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

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
    fn ignores_division_that_isnt_nan() {
        let diags = run("local same = x == 0/1");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_self_comparison_nan_check() {
        let diags = run("local is_nan = x ~= x");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
