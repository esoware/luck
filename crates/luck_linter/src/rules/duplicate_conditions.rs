use core::iter;

use luck_ast::{AstKind, AstType, ContentEq as _, Expression, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DuplicateConditions;

declare_lint_rule!(
    DuplicateConditions,
    Correctness,
    Warning,
    "`if`/`elseif` chain has duplicate conditions"
);

impl Rule for DuplicateConditions {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::IfStatement]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::IfStatement(if_stmt) = node.kind() else {
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
            let Some(first) =
                earlier.iter().find(|&&earlier_condition| earlier_condition.content_eq(condition))
            else {
                earlier.push(condition);
                continue;
            };
            let span = condition.span();
            let source_text = String::from_utf8_lossy(span.text(context.source));
            let text = source_text.trim();
            context.diagnostic(
                Finding::new(format!("duplicate condition `{text}` in `if`/`elseif` chain"), span)
                    .with_label(first.span(), "first checked here")
                    .with_help("this branch is unreachable unless the condition has side effects"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::DuplicateConditions, source, LuaVersion::Lua54)
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
