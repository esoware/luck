use luck_ast::Expression;
use luck_ast::node::{AstTypesBitset, NodeType};
use luck_token::UnOp;

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

fn extract_number(expr: &Expression) -> Option<f64> {
    match expr {
        Expression::Number(literal) => literal.text.parse().ok(),
        Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Neg) => {
            extract_number(&unop.operand).map(|value| -value)
        }
        _ => None,
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
        _ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::NumericFor(num_for) = stmt {
            let start = extract_number(&num_for.start);
            let limit = extract_number(&num_for.limit);

            if let (Some(start), Some(limit)) = (start, limit)
                && start > limit
                && num_for.step.is_none()
            {
                out.push(
                    LintDiagnostic::new(
                        "reversed_for_loop",
                        format!(
                            "for loop from {start} to {limit} without step -1 will never execute"
                        ),
                        num_for.span,
                    )
                    .with_help("add a negative step: `for i = start, limit, -1 do`".to_string()),
                );
            }
        }
    }
}
