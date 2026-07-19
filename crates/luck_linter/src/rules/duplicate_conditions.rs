use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct DuplicateConditions;

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
        "if/elseif chain has duplicate conditions"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn expression_source(source: &str, span: luck_token::Span) -> &str {
    let start = span.start as usize;
    let end = span.end as usize;
    if start <= end && end <= source.len() {
        source[start..end].trim()
    } else {
        ""
    }
}

impl NodeRule for DuplicateConditions {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::IfStatement]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::stmt::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::IfStatement(if_stmt) = stmt {
            let condition_span = if_stmt.condition.span();
            let if_condition_text = expression_source(ctx.source, condition_span);

            if !if_condition_text.is_empty() {
                let mut conditions: Vec<(&str, luck_token::Span)> =
                    vec![(if_condition_text, condition_span)];

                for clause in &if_stmt.elseif_clauses {
                    let clause_span = clause.condition.span();
                    let clause_text = expression_source(ctx.source, clause_span);
                    conditions.push((clause_text, clause_span));
                }

                for index in 1..conditions.len() {
                    let (current_text, current_span) = conditions[index];
                    if current_text.is_empty() {
                        continue;
                    }
                    for &(earlier_text, _) in conditions.iter().take(index) {
                        if current_text == earlier_text {
                            out.push(LintDiagnostic::new("duplicate_conditions", format!(
                                    "duplicate condition '{}' in if/elseif chain",
                                    current_text
                                ), current_span).with_help(
                                    "this branch can never execute because the same condition was already checked".to_string()
                                ));
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&DuplicateConditions, source, LuaVersion::Lua54)
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
