use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

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
        let block = ctx.block;
        let _semantic = ctx.semantic;
        let source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = SameThenElseChecker {
            source,
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct SameThenElseChecker<'a> {
    source: &'a str,
    diagnostics: Vec<LintDiagnostic>,
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

impl Visitor for SameThenElseChecker<'_> {
    fn visit_statement(&mut self, stmt: &luck_ast::Statement) {
        if let luck_ast::Statement::IfStatement(if_stmt) = stmt
            && let Some(else_clause) = &if_stmt.else_clause
        {
            let then_src = block_source(self.source, &if_stmt.block);
            let else_src = block_source(self.source, &else_clause.block);

            if !then_src.is_empty() && then_src.trim() == else_src.trim() {
                self.diagnostics.push(
                    LintDiagnostic::new(
                        "if_same_then_else",
                        "if and else branches have identical bodies".to_string(),
                        if_stmt.span,
                    )
                    .with_help("the condition has no effect; consider removing the if".to_string()),
                );
            }
        }
        self.walk_statement(stmt);
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
