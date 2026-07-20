use luck_ast::Expression;
use luck_token::BinOp;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct CompareNan;

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
        "comparison with NaN (0/0) always fails"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn is_nan_expr(expr: &Expression) -> bool {
    if let Expression::BinaryOp(binop) = expr
        && binop.op == BinOp::Div
    {
        return is_zero(&binop.left) && is_zero(&binop.right);
    }
    false
}

fn is_zero(expr: &Expression) -> bool {
    matches!(expr, Expression::Number(literal) if literal.text == "0")
}

impl NodeRule for CompareNan {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::BinaryOp]);
        Some(&TYPES)
    }
    fn on_expression(&self, expr: &Expression, _ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::BinaryOp(binop) = expr
            && matches!(binop.op, BinOp::Eq | BinOp::Ne)
            && (is_nan_expr(&binop.left) || is_nan_expr(&binop.right))
        {
            out.push(
                LintDiagnostic::new(
                    "compare_nan",
                    "comparison with NaN (0/0) always fails; use x ~= x to check for NaN"
                        .to_string(),
                    binop.span,
                )
                .with_help("use `x ~= x` instead".to_string()),
            );
        }
    }
}
