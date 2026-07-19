use luck_ast::Expression;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct ParenthesizedConditions;

impl Rule for ParenthesizedConditions {
    fn name(&self) -> &'static str {
        "parenthesized_conditions"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "unnecessary parentheses around condition"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn paren_fix(source: &str, expr: &Expression) -> Option<Fix> {
    if let Expression::Parenthesized(paren) = expr {
        let inner_span = paren.expr.span();
        let inner_text = &source[inner_span.start as usize..inner_span.end as usize];
        Some(Fix {
            description: "remove unnecessary parentheses".to_string(),
            edits: vec![TextEdit {
                span: paren.span,
                replacement: inner_text.to_string(),
            }],
        })
    } else {
        None
    }
}

fn is_parenthesized(expr: &Expression) -> bool {
    matches!(expr, Expression::Parenthesized(_))
}

impl NodeRule for ParenthesizedConditions {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset =
            AstTypesBitset::from_types(&[NodeType::IfStatement, NodeType::WhileLoop]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::stmt::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        match stmt {
            luck_ast::Statement::IfStatement(if_stmt) => {
                if is_parenthesized(&if_stmt.condition) {
                    out.push(
                        LintDiagnostic::new(
                            "parenthesized_conditions",
                            "unnecessary parentheses around if condition".to_string(),
                            if_stmt.span,
                        )
                        .with_help("Lua does not require parentheses around conditions".to_string())
                        .with_fix_opt(paren_fix(ctx.source, &if_stmt.condition)),
                    );
                }
                for clause in &if_stmt.elseif_clauses {
                    if is_parenthesized(&clause.condition) {
                        out.push(
                            LintDiagnostic::new(
                                "parenthesized_conditions",
                                "unnecessary parentheses around elseif condition".to_string(),
                                clause.span,
                            )
                            .with_fix_opt(paren_fix(ctx.source, &clause.condition)),
                        );
                    }
                }
            }
            luck_ast::Statement::WhileLoop(while_loop)
                if is_parenthesized(&while_loop.condition) =>
            {
                out.push(
                    LintDiagnostic::new(
                        "parenthesized_conditions",
                        "unnecessary parentheses around while condition".to_string(),
                        while_loop.span,
                    )
                    .with_fix_opt(paren_fix(ctx.source, &while_loop.condition)),
                );
            }
            _ => {}
        }
    }
}
