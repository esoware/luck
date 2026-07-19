use luck_ast::Expression;
use luck_ast::node::{AstTypesBitset, NodeType};

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

pub struct ReversedForLoop;

impl Rule for ReversedForLoop {
    fn name(&self) -> &'static str {
        "reversed_for_loop"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "numeric for loop counts down without negative step"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn extract_number(expr: &Expression, source: &str) -> Option<f64> {
    if let Expression::Number(token) = expr {
        let text = &source[token.span.start as usize..token.span.end as usize];
        text.parse().ok()
    } else if let Expression::UnaryOp(unop) = expr {
        if let luck_token::TokenKind::Minus = &unop.op.kind {
            extract_number(&unop.operand, source).map(|n| -n)
        } else {
            None
        }
    } else {
        None
    }
}

impl NodeRule for ReversedForLoop {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::NumericFor]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::NumericFor(num_for) = stmt {
            let start = extract_number(&num_for.start, ctx.source);
            let limit = extract_number(&num_for.limit, ctx.source);

            if let (Some(s), Some(l)) = (start, limit)
                && s > l
                && num_for.comma2_and_step.is_none()
            {
                out.push(
                    LintDiagnostic::new(
                        "reversed_for_loop",
                        format!("for loop from {s} to {l} without step -1 will never execute"),
                        num_for.span,
                    )
                    .with_help("add a negative step: `for i = start, limit, -1 do`".to_string()),
                );
            }
        }
    }
}
