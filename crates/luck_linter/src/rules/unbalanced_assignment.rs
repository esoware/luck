use luck_ast::{AstKind, AstType, is_multi_value};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnbalancedAssignment;

impl_rule!(
    UnbalancedAssignment,
    "unbalanced_assignment",
    Suspicious,
    Warning,
    "assignment has a different number of targets and values"
);

impl RuleRunner for UnbalancedAssignment {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[AstType::LocalAssignment, AstType::Assignment])
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let kind = node.kind();
        if let AstKind::LocalAssignment(local) = kind {
            let name_count = local.names.len();
            let value_count = local.exprs.len();
            if !local.exprs.last().is_some_and(is_multi_value)
                && name_count != value_count
                && value_count > 0
            {
                context.diagnostic(Finding::new(
                    format!("{name_count} name(s) but {value_count} value(s) in assignment"),
                    local.span,
                ));
            }
            return;
        }
        if let AstKind::Assignment(assignment) = kind {
            let target_count = assignment.targets.len();
            let value_count = assignment.values.len();
            if !assignment.values.last().is_some_and(is_multi_value) && target_count != value_count
            {
                context.diagnostic(Finding::new(
                    format!("{target_count} target(s) but {value_count} value(s) in assignment"),
                    assignment.span,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
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
        let diags = run("\
local a, b
a, b = 1, 2, 3");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "2 target(s) but 3 value(s) in assignment", "{diags:?}");
    }

    #[test]
    fn ignores_multi_value_tail() {
        let diags = run("\
local a, b, c = 1, f()
local function g(...) a, b, c = ... end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_local_without_values() {
        let diags = run("local a, b");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_balanced_assignments() {
        let diags = run("\
local a, b = 1, 2
a, b = b, a");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
