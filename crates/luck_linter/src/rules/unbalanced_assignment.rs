use luck_ast::Expression;
use luck_ast::node::{AstTypesBitset, NodeType};

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

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
        crate::bus::run_single(self, ctx)
    }
}

fn last_is_multiret(punct: &luck_ast::shared::Punctuated<Expression>) -> bool {
    matches!(
        punct.last(),
        Some(Expression::FunctionCall(_)) | Some(Expression::VarArg(_))
    )
}

impl NodeRule for UnbalancedAssignment {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset =
            AstTypesBitset::from_types(&[NodeType::LocalAssignment, NodeType::Assignment]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::Statement,
        _ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        match stmt {
            luck_ast::Statement::LocalAssignment(local) => {
                if let Some(exprs) = &local.exprs {
                    let names = local.names.len();
                    let values = exprs.len();
                    if !last_is_multiret(exprs) && names != values && values > 0 {
                        out.push(LintDiagnostic::new(
                            "unbalanced_assignment",
                            format!("{names} name(s) but {values} value(s) in assignment"),
                            local.span,
                        ));
                    }
                }
            }
            luck_ast::Statement::Assignment(assign) => {
                let targets = assign.targets.len();
                let values = assign.values.len();
                if !last_is_multiret(&assign.values) && targets != values {
                    out.push(LintDiagnostic::new(
                        "unbalanced_assignment",
                        format!("{targets} target(s) but {values} value(s) in assignment"),
                        assign.span,
                    ));
                }
            }
            _ => {}
        }
    }
}
