use luck_ast::Expression;
use luck_ast::visitor::Visitor;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

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
        let block = ctx.block;
        let _semantic = ctx.semantic;
        let source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = ReversedForChecker {
            source,
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct ReversedForChecker<'a> {
    source: &'a str,
    diagnostics: Vec<LintDiagnostic>,
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

impl Visitor for ReversedForChecker<'_> {
    fn visit_statement(&mut self, stmt: &luck_ast::Statement) {
        if let luck_ast::Statement::NumericFor(num_for) = stmt {
            let start = extract_number(&num_for.start, self.source);
            let limit = extract_number(&num_for.limit, self.source);

            if let (Some(s), Some(l)) = (start, limit)
                && s > l
                && num_for.comma2_and_step.is_none()
            {
                self.diagnostics.push(
                    LintDiagnostic::new(
                        "reversed_for_loop",
                        format!("for loop from {s} to {l} without step -1 will never execute"),
                        num_for.span,
                    )
                    .with_help("add a negative step: `for i = start, limit, -1 do`".to_string()),
                );
            }
        }
        self.walk_statement(stmt);
    }
}
