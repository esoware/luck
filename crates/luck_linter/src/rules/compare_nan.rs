use luck_ast::Expression;
use luck_token::TokenKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

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

fn is_nan_expr(expr: &Expression, source: &str) -> bool {
    if let Expression::BinaryOp(binop) = expr
        && let TokenKind::Slash = &binop.op.kind
    {
        return is_zero(&binop.left, source) && is_zero(&binop.right, source);
    }
    false
}

fn is_zero(expr: &Expression, source: &str) -> bool {
    if let Expression::Number(token) = expr {
        let text = &source[token.span.start as usize..token.span.end as usize];
        return text == "0";
    }
    false
}

impl NodeRule for CompareNan {
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::BinaryOp(binop) = expr
            && matches!(binop.op.kind, TokenKind::EqualEqual | TokenKind::TildeEqual)
            && (is_nan_expr(&binop.left, ctx.source) || is_nan_expr(&binop.right, ctx.source))
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
