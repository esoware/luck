use std::iter;

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
            Vec::with_capacity(if_stmt.elseif_clauses.len() + 1);
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
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "if a then x() elseif b then y() end",
        "if a then x() end",
        "if a == 1 then x() elseif a == 2 then y() elseif a.b == 1 then z() end",
    ];

    const FAIL: &[&str] = &[
        "if a then print(1) elseif a then print(2) end",
        "if a then x() elseif b then y() elseif a then z() end",
        "if a==1 then x() elseif a == 1 then y() elseif ( a ) then z() end",
        "\
if c then x()
elseif c0 then x()
elseif c1 then x()
elseif c0 then x()
elseif c1 then x()
end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::DuplicateConditions, PASS, FAIL).test_and_snapshot();
    }
}
