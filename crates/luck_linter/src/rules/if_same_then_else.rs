use luck_ast::Statement;

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct IfSameThenElse;

impl_rule!(
    IfSameThenElse,
    "if_same_then_else",
    Suspicious,
    Warning,
    "`if` and `else` branches have identical bodies"
);

impl NodeRule for IfSameThenElse {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::IfStatement])
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let Statement::IfStatement(if_stmt) = stmt else {
            return;
        };
        let Some(else_clause) = &if_stmt.else_clause else {
            return;
        };
        let then_text = if_stmt.block.span.text(context.source);
        let else_text = else_clause.block.span.text(context.source);
        if !then_text.is_empty() && then_text.trim() == else_text.trim() {
            let finding =
                Finding::new("`if` and `else` branches have identical bodies", if_stmt.span);
            // An `elseif` keeps the first condition meaningful.
            findings.push(if if_stmt.elseif_clauses.is_empty() {
                finding.with_help("the condition has no effect, so remove the `if`")
            } else {
                finding
            });
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
        run_rule(&IfSameThenElse, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_identical_call_branches() {
        let diags = run("if a then print(1) else print(1) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_identical_assignment_branches() {
        let diags = run("if cond then x = 1 else x = 1 end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_different_branches() {
        let diags = run("if a then print(1) else print(2) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_if_without_else() {
        let diags = run("if a then print(1) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_identical_branches_with_elseif_without_dead_condition_help() {
        let diags = run("if a then f() elseif b then g() else f() end");
        assert!(
            diags.iter().all(|diag| !diag.help.as_deref().unwrap_or("").contains("no effect")),
            "the condition decides whether `elseif` runs, so removing the `if` isn't safe: {diags:?}"
        );
    }
}
