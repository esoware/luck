use luck_ast::node::{AstTypesBitset, NodeType};
use luck_ast::shared::Block;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

pub struct IfSameThenElse;

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
        "if and else branches have identical bodies"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn block_source<'a>(source: &'a str, block: &Block) -> &'a str {
    let start = block.span.start as usize;
    let end = block.span.end as usize;
    if start <= end && end <= source.len() {
        &source[start..end]
    } else {
        ""
    }
}

impl NodeRule for IfSameThenElse {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::IfStatement]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::Statement,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::IfStatement(if_stmt) = stmt
            && let Some(else_clause) = &if_stmt.else_clause
        {
            let then_src = block_source(ctx.source, &if_stmt.block);
            let else_src = block_source(ctx.source, &else_clause.block);

            if !then_src.is_empty() && then_src.trim() == else_src.trim() {
                out.push(
                    LintDiagnostic::new(
                        "if_same_then_else",
                        "if and else branches have identical bodies".to_string(),
                        if_stmt.span,
                    )
                    .with_help("the condition has no effect; consider removing the if".to_string()),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&IfSameThenElse, source, LuaVersion::Lua54)
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
