use luck_ast::Statement;

use crate::call::has_multi_value_tail;
use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnbalancedAssignment;

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
        "assignment has a different number of targets and values"
    }
}

impl NodeRule for UnbalancedAssignment {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::LocalAssignment, NodeType::Assignment])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        match stmt {
            Statement::LocalAssignment(local) => {
                let Some(exprs) = &local.exprs else {
                    return;
                };
                let name_count = local.names.len();
                let value_count = exprs.len();
                if !has_multi_value_tail(&exprs.items)
                    && name_count != value_count
                    && value_count > 0
                {
                    findings.push(Finding::new(
                        format!("{name_count} name(s) but {value_count} value(s) in assignment"),
                        local.span,
                    ));
                }
            }
            Statement::Assignment(assignment) => {
                let target_count = assignment.targets.len();
                let value_count = assignment.values.len();
                if !has_multi_value_tail(&assignment.values.items) && target_count != value_count {
                    findings.push(Finding::new(
                        format!(
                            "{target_count} target(s) but {value_count} value(s) in assignment"
                        ),
                        assignment.span,
                    ));
                }
            }
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&UnbalancedAssignment, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_local_with_fewer_values() {
        let diags = run("local a, b, c = 1, 2");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "3 name(s) but 2 value(s) in assignment", "{diags:?}");
    }

    #[test]
    fn flags_assignment_with_extra_values() {
        let diags = run("local a, b\na, b = 1, 2, 3");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "2 target(s) but 3 value(s) in assignment", "{diags:?}");
    }

    #[test]
    fn ignores_multi_value_tail() {
        let diags = run("local a, b, c = 1, f()\nlocal function g(...) a, b, c = ... end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_local_without_values() {
        let diags = run("local a, b");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_balanced_assignments() {
        let diags = run("local a, b = 1, 2\na, b = b, a");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
