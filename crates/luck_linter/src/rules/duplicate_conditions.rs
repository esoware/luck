use core::iter;

use luck_ast::{ContentEq as _, Expression, GetSpan as _, Statement};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateConditions;

impl_rule!(
    DuplicateConditions,
    "duplicate_conditions",
    Correctness,
    Warning,
    "`if`/`elseif` chain has duplicate conditions"
);

impl NodeRule for DuplicateConditions {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::IfStatement])
    }
    fn on_stmt(
        &self,
        stmt: &Statement<'_>,
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
        // A condition the parser found missing repeats nothing.
        let conditions = iter::once(&if_stmt.condition)
            .chain(if_stmt.elseif_clauses.iter().map(|clause| &clause.condition))
            .filter(|condition| !matches!(condition, Expression::Error(_)));
        let mut earlier: Vec<&Expression<'_>> =
            Vec::with_capacity(if_stmt.elseif_clauses.len().saturating_add(1));
        for condition in conditions {
            if !earlier.iter().any(|&earlier_condition| earlier_condition.content_eq(condition)) {
                earlier.push(condition);
                continue;
            }
            let span = condition.span();
            let text = span.text(context.source).trim();
            findings.push(
                Finding::new(format!("duplicate condition `{text}` in `if`/`elseif` chain"), span)
                    .with_help(
                        "the same condition is checked earlier, so this branch is unreachable unless the condition has side effects",
                    ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

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

    #[test]
    fn flags_a_repeat_spelled_with_other_spacing() {
        let diags = run("if a==1 then x() elseif a == 1 then y() elseif ( a ) then z() end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_conditions_that_differ_only_inside() {
        let diags = run("if a == 1 then x() elseif a == 2 then y() elseif a.b == 1 then z() end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_each_repeat_in_a_long_chain() {
        let mut arms = String::new();
        for i in 0..2_000u32 {
            let _unused = write!(arms, " elseif c{i} then x()");
        }
        let diags = run(&format!("if c then x(){arms}{arms} end"));
        assert_eq!(diags.len(), 2_000, "{diags:?}");
    }
}
