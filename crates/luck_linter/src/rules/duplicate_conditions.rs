use core::iter;

use luck_ast::Statement;
use luck_token::Span;

use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateConditions;

impl Rule for DuplicateConditions {
    fn name(&self) -> &'static str {
        "duplicate_conditions"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "`if`/`elseif` chain has duplicate conditions"
    }
}

impl NodeRule for DuplicateConditions {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::IfStatement])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let Statement::IfStatement(if_stmt) = stmt else {
            return;
        };
        // Only an `elseif` can repeat an earlier condition.
        if if_stmt.elseif_clauses.is_empty() {
            return;
        }
        // Conditions compare by their source text.
        let conditions: Vec<(&str, Span)> = iter::once(&if_stmt.condition)
            .chain(if_stmt.elseif_clauses.iter().map(|clause| &clause.condition))
            .map(|condition| {
                let span = condition.span();
                (span.text(context.source).trim(), span)
            })
            .collect();
        if conditions.first().is_none_or(|(if_text, _)| if_text.is_empty()) {
            return;
        }
        for (index, &(text, span)) in conditions.iter().enumerate().skip(1) {
            if !text.is_empty()
                && conditions.iter().take(index).any(|&(earlier_text, _)| earlier_text == text)
            {
                findings.push(
                    Finding::new(
                        format!("duplicate condition `{text}` in `if`/`elseif` chain"),
                        span,
                    )
                    .with_help(
                        "the same condition is checked earlier, so this branch is unreachable unless the condition has side effects",
                    ),
                );
            }
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
        run_rule(&DuplicateConditions, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_duplicate_elseif() {
        let diags = run("if a then print(1) elseif a then print(2) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_duplicate_in_longer_chain() {
        let diags = run("if a then x() elseif b then y() elseif a then z() end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_distinct_conditions() {
        let diags = run("if a then x() elseif b then y() end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_if_without_elseif() {
        let diags = run("if a then x() end");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
