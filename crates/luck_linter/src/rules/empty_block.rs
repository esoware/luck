use luck_ast::shared::Block;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

pub struct EmptyBlock;

impl Rule for EmptyBlock {
    fn name(&self) -> &'static str {
        "empty_block"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "block body is empty"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn is_empty_block(block: &Block) -> bool {
    block.stmts.is_empty() && block.last_stmt.is_none()
}

impl NodeRule for EmptyBlock {
    fn on_statement(
        &self,
        stmt: &luck_ast::stmt::Statement,
        _ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        match stmt {
            luck_ast::Statement::IfStatement(if_stmt) => {
                if is_empty_block(&if_stmt.block) {
                    out.push(LintDiagnostic::new(
                        "empty_block",
                        "empty if block".to_string(),
                        if_stmt.span,
                    ));
                }
                for clause in &if_stmt.elseif_clauses {
                    if is_empty_block(&clause.block) {
                        out.push(LintDiagnostic::new(
                            "empty_block",
                            "empty elseif block".to_string(),
                            clause.span,
                        ));
                    }
                }
                if let Some(else_clause) = &if_stmt.else_clause
                    && is_empty_block(&else_clause.block)
                {
                    out.push(LintDiagnostic::new(
                        "empty_block",
                        "empty else block".to_string(),
                        else_clause.span,
                    ));
                }
            }
            luck_ast::Statement::WhileLoop(while_loop) if is_empty_block(&while_loop.block) => {
                out.push(LintDiagnostic::new(
                    "empty_block",
                    "empty while loop body".to_string(),
                    while_loop.span,
                ));
            }
            luck_ast::Statement::DoBlock(do_block) if is_empty_block(&do_block.block) => {
                out.push(LintDiagnostic::new(
                    "empty_block",
                    "empty do block".to_string(),
                    do_block.span,
                ));
            }
            luck_ast::Statement::NumericFor(for_loop) if is_empty_block(&for_loop.block) => {
                out.push(LintDiagnostic::new(
                    "empty_block",
                    "empty numeric for loop body".to_string(),
                    for_loop.span,
                ));
            }
            luck_ast::Statement::GenericFor(for_loop) if is_empty_block(&for_loop.block) => {
                out.push(LintDiagnostic::new(
                    "empty_block",
                    "empty generic for loop body".to_string(),
                    for_loop.span,
                ));
            }
            luck_ast::Statement::RepeatLoop(repeat_loop) if is_empty_block(&repeat_loop.block) => {
                out.push(LintDiagnostic::new(
                    "empty_block",
                    "empty repeat loop body".to_string(),
                    repeat_loop.span,
                ));
            }
            _ => {}
        }
    }
}
