use luck_ast::{Expression, Statement};
use luck_token::{Span, offset_to_index};

use crate::diagnostic::Finding;
use crate::fix::{Fix, TextEdit};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ParenthesizedConditions;

impl_rule!(
    ParenthesizedConditions,
    "parenthesized_conditions",
    Style,
    Warning,
    "unnecessary parentheses around condition"
);

impl NodeRule for ParenthesizedConditions {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::IfStatement, NodeType::WhileLoop])
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let source = context.source;
        match stmt {
            Statement::IfStatement(if_stmt) => {
                findings.extend(
                    check_condition(
                        &if_stmt.condition,
                        "unnecessary parentheses around `if` condition",
                        if_stmt.span,
                        source,
                    )
                    .map(|finding| {
                        finding.with_help("Lua doesn't need parentheses around conditions")
                    }),
                );
                for clause in &if_stmt.elseif_clauses {
                    findings.extend(check_condition(
                        &clause.condition,
                        "unnecessary parentheses around `elseif` condition",
                        clause.span,
                        source,
                    ));
                }
            }
            Statement::WhileLoop(while_loop) => {
                findings.extend(check_condition(
                    &while_loop.condition,
                    "unnecessary parentheses around `while` condition",
                    while_loop.span,
                    source,
                ));
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::RepeatLoop(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
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

/// The fix deletes just those two tokens, so anything between them stays.
fn check_condition(
    condition: &Expression,
    message: &str,
    span: Span,
    source: &str,
) -> Option<Finding> {
    let Expression::Parenthesized(parenthesized) = condition else {
        return None;
    };
    let Span { start, end } = parenthesized.span;
    let bytes = source.as_bytes();
    let before = offset_to_index(start).checked_sub(1).and_then(|i| bytes.get(i));
    let after = bytes.get(offset_to_index(end));
    Some(Finding::new(message, span).with_fix(Fix {
        description: "remove unnecessary parentheses".to_owned(),
        edits: vec![
            TextEdit {
                span: Span::new(start, start.saturating_add(1)),
                replacement: replacement_beside(before),
            },
            TextEdit {
                span: Span::new(end.saturating_sub(1), end),
                replacement: replacement_beside(after),
            },
        ],
    }))
}

/// What a deleted parenthesis leaves: a space if the byte beside it would
/// otherwise run into the condition.
fn replacement_beside(neighbor: Option<&u8>) -> String {
    let is_word_byte = neighbor.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
    if is_word_byte { " ".to_owned() } else { String::new() }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&ParenthesizedConditions, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_each_parenthesized_condition() {
        let diags = run("\
local c = 1
if (c) then elseif (c) then end
while (c) do break end");
        let messages: Vec<&str> = diags.iter().map(|diag| diag.message.as_str()).collect();
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
while (a) == b do break end");
        assert!(diags.is_empty(), "{diags:?}");
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
