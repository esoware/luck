use luck_ast::{NodeType, NodeTypeSet, Statement};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct IfSameThenElse;

impl Rule for IfSameThenElse {
    fn name(&self) -> &'static str {
        "if_same_then_else"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "`if` and `else` branches have identical bodies"
    }
}

impl NodeRule for IfSameThenElse {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::IfStatement])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        let Statement::IfStatement(if_stmt) = stmt else {
            return;
        };
        let Some(else_clause) = &if_stmt.else_clause else {
            return;
        };
        let then_text = if_stmt.block.span.text(context.source);
        let else_text = else_clause.block.span.text(context.source);
        if !then_text.is_empty() && then_text.trim() == else_text.trim() {
            diags.push(
                LintDiagnostic::new(
                    "if_same_then_else",
                    "`if` and `else` branches have identical bodies",
                    if_stmt.span,
                )
                .with_help("the condition has no effect, so remove the `if`"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
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
}
