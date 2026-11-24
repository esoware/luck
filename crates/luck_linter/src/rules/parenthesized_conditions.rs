use luck_ast::{AstKind, AstType, Expression, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::Span;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::fix::{FixKind, TextEdit};
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ParenthesizedConditions;

declare_lint_rule!(
    ParenthesizedConditions,
    Style,
    Warning,
    "unnecessary parentheses around condition",
    fix = Fix
);

impl Rule for ParenthesizedConditions {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[
        AstType::IfStatement,
        AstType::ElseIfClause,
        AstType::WhileLoop,
    ]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let kind = node.kind();
        let found = if let AstKind::IfStatement(if_stmt) = kind {
            check_condition(
                &if_stmt.condition,
                "unnecessary parentheses around `if` condition",
                if_stmt.span,
                context,
            )
            .map(|finding| finding.with_help("Lua doesn't need parentheses around conditions"))
        } else if let AstKind::ElseIfClause(clause) = kind {
            check_condition(
                &clause.condition,
                "unnecessary parentheses around `elseif` condition",
                clause.span,
                context,
            )
        } else if let AstKind::WhileLoop(while_loop) = kind {
            check_condition(
                &while_loop.condition,
                "unnecessary parentheses around `while` condition",
                while_loop.span,
                context,
            )
        } else {
            None
        };
        if let Some(finding) = found {
            context.diagnostic(finding);
        }
    }
}

/// The fix deletes just the outermost pair, so anything between them stays.
/// A condition takes one value, so the parentheses that truncate a call to
/// one change nothing there either.
fn check_condition(
    condition: &Expression<'_>,
    message: &'static str,
    span: Span,
    context: &LintContext<'_>,
) -> Option<Finding> {
    let Span { start, end } = if matches!(condition, Expression::Parenthesized(_)) {
        context.extent(condition)
    } else {
        context.groupings.around(condition.span())?
    };
    Some(Finding::new(message, span).with_fix(context, FixKind::Fix, |fixer| {
        let bytes = context.source;
        let before = bytes[..start as usize].last();
        let after = bytes.get(end as usize);
        fixer.fix(
            "remove unnecessary parentheses",
            [
                TextEdit {
                    span: Span::new(start, start + 1),
                    replacement: replacement_beside(before).into(),
                },
                TextEdit {
                    span: Span::new(end - 1, end),
                    replacement: replacement_beside(after).into(),
                },
            ],
        )
    }))
}

/// What a deleted parenthesis leaves: a space if the byte beside it would
/// otherwise run into the condition.
fn replacement_beside(neighbor: Option<&u8>) -> &'static [u8] {
    let is_word_byte = neighbor.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
    if is_word_byte { b" " } else { b"" }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &["\
local a, b = 1, 2
if a then end
while (a) == b do break end
if (a).b then end
if (f)() then end"];

    const FAIL: &[&str] = &[
        "\
local c = 1
if (c) then elseif (c) then end
while (c) do break end",
        "\
if (f()) then end
while ((c)) do break end",
        "\
local c = 1
if (c == 1) then end",
        // The fix should keep the tokens apart.
        "if(x)then y() end",
        // The fix shouldn't delete the comment.
        "if (--[[why]] x) then end",
        "\
local c = 1
if c then elseif(c)then end
while(c)do break end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::ParenthesizedConditions, PASS, FAIL).test_and_snapshot();
    }
}
