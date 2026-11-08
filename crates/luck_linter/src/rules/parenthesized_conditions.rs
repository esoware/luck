use luck_ast::{Expression, NodeType, NodeTypeSet, Statement};
use luck_token::Span;

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::fix::{Fix, TextEdit};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ParenthesizedConditions;

impl Rule for ParenthesizedConditions {
    fn name(&self) -> &'static str {
        "parenthesized_conditions"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "unnecessary parentheses around condition"
    }
}

impl NodeRule for ParenthesizedConditions {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::IfStatement, NodeType::WhileLoop])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        let source = context.source;
        match stmt {
            Statement::IfStatement(if_stmt) => {
                diags.extend(
                    check_condition(
                        &if_stmt.condition,
                        "unnecessary parentheses around `if` condition",
                        if_stmt.span,
                        source,
                    )
                    .map(|diag| diag.with_help("Lua doesn't need parentheses around conditions")),
                );
                for clause in &if_stmt.elseif_clauses {
                    diags.extend(check_condition(
                        &clause.condition,
                        "unnecessary parentheses around `elseif` condition",
                        clause.span,
                        source,
                    ));
                }
            }
            Statement::WhileLoop(while_loop) => {
                diags.extend(check_condition(
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

/// Flags `span` when `condition` is wrapped in parentheses, with a fix that
/// keeps only the inner expression's text.
fn check_condition(
    condition: &Expression,
    message: &str,
    span: Span,
    source: &str,
) -> Option<LintDiagnostic> {
    let Expression::Parenthesized(parenthesized) = condition else {
        return None;
    };
    Some(LintDiagnostic::new("parenthesized_conditions", message, span).with_fix(Fix {
        description: "remove unnecessary parentheses".to_owned(),
        edits: vec![TextEdit {
            span: parenthesized.span,
            replacement: parenthesized.expr.span().text(source).to_owned(),
        }],
    }))
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&ParenthesizedConditions, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_each_parenthesized_condition() {
        let diags = run("local c = 1\nif (c) then elseif (c) then end\nwhile (c) do break end");
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
        let diags = run("local a, b = 1, 2\nif a then end\nwhile (a) == b do break end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_parenthesized_comparison() {
        let source = "local c = 1\nif (c == 1) then end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local c = 1\nif c == 1 then end");
    }
}
