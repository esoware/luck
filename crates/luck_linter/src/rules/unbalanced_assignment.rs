use luck_ast::Expression;
use luck_ast::visitor::Visitor;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct UnbalancedAssignment;

impl Rule for UnbalancedAssignment {
    fn name(&self) -> &'static str {
        "unbalanced_assignment"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "assignment has different number of targets and values"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let _semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = UnbalancedChecker {
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct UnbalancedChecker {
    diagnostics: Vec<LintDiagnostic>,
}

fn count_punctuated_exprs(punct: &luck_ast::shared::Punctuated<Expression>) -> usize {
    punct.len()
}

fn last_is_multiret(punct: &luck_ast::shared::Punctuated<Expression>) -> bool {
    matches!(
        punct.last_item(),
        Some(Expression::FunctionCall(_)) | Some(Expression::VarArg(_))
    )
}

impl Visitor for UnbalancedChecker {
    fn visit_statement(&mut self, stmt: &luck_ast::Statement) {
        match stmt {
            luck_ast::Statement::LocalAssignment(local) => {
                if let Some((_, exprs)) = &local.equal_and_exprs {
                    let names = local.names.len();
                    let values = count_punctuated_exprs(exprs);
                    if !last_is_multiret(exprs) && names != values && values > 0 {
                        self.diagnostics.push(LintDiagnostic::new(
                            "unbalanced_assignment",
                            format!("{names} name(s) but {values} value(s) in assignment"),
                            local.span,
                        ));
                    }
                }
            }
            luck_ast::Statement::Assignment(assign) => {
                let targets = assign.targets.len();
                let values = count_punctuated_exprs(&assign.values);
                if !last_is_multiret(&assign.values) && targets != values {
                    self.diagnostics.push(LintDiagnostic::new(
                        "unbalanced_assignment",
                        format!("{targets} target(s) but {values} value(s) in assignment"),
                        assign.span,
                    ));
                }
            }
            _ => {}
        }
        self.walk_statement(stmt);
    }
}
