use luck_ast::{AstKind, AstType, Expression, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{Span, offset_to_index};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::fix::{FixKind, TextEdit};
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ParenthesizedConditions;

impl_rule!(
    ParenthesizedConditions,
    "parenthesized_conditions",
    Style,
    Warning,
    "unnecessary parentheses around condition",
    Fix
);

impl RuleRunner for ParenthesizedConditions {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[
            AstType::IfStatement,
            AstType::ElseIfClause,
            AstType::WhileLoop,
        ])
    }

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
        let before = offset_to_index(start).checked_sub(1).and_then(|i| bytes.get(i));
        let after = bytes.get(offset_to_index(end));
        fixer.fix(
            "remove unnecessary parentheses",
            [
                TextEdit {
                    span: Span::new(start, start.saturating_add(1)),
                    replacement: replacement_beside(before).into(),
                },
                TextEdit {
                    span: Span::new(end.saturating_sub(1), end),
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
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<Message> {
        run_rule(&ParenthesizedConditions, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_each_parenthesized_condition() {
        let diags = run("\
local c = 1
if (c) then elseif (c) then end
while (c) do break end");
        let messages: Vec<&str> = diags.iter().map(|diag| &*diag.message).collect();
        assert_eq!(
            messages,
            [
                "unnecessary parentheses around `if` condition",
                "unnecessary parentheses around `elseif` condition",
                "unnecessary parentheses around `while` condition",
            ],
            "{diags:?}"
        );
    }

    #[test]
    fn ignores_bare_and_partly_parenthesized_conditions() {
        let diags = run("\
local a, b = 1, 2
if a then end
while (a) == b do break end
if (a).b then end
if (f)() then end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_a_truncated_call_and_the_outer_pair() {
        let source = "\
if (f()) then end
while ((c)) do break end";
        let diags = run(source);
        assert_eq!(diags.len(), 2, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(
            fixed,
            "\
if f() then end
while (c) do break end"
        );
    }

    #[test]
    fn flags_and_fixes_parenthesized_comparison() {
        let source = "\
local c = 1
if (c == 1) then end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(
            fixed,
            "\
local c = 1
if c == 1 then end"
        );
    }

    #[test]
    fn flags_and_fixes_keeping_keywords_separated() {
        let source = "if(x)then y() end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "if x then y() end", "the fix should keep the tokens apart");
    }

    #[test]
    fn flags_and_fixes_keeping_comment() {
        let source = "if (--[[why]] x) then end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert!(fixed.contains("--[[why]]"), "the fix shouldn't delete the comment: {fixed}");
    }

    #[test]
    fn flags_and_fixes_elseif_and_while_keywords_glued_to_parens() {
        let source = "\
local c = 1
if c then elseif(c)then end
while(c)do break end";
        let diags = run(source);
        assert_eq!(diags.len(), 2, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(
            fixed,
            "\
local c = 1
if c then elseif c then end
while c do break end"
        );
    }
}
