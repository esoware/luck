use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct UnreachableCode;

impl Rule for UnreachableCode {
    fn name(&self) -> &'static str {
        "unreachable_code"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "code after return/break/continue is unreachable"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let _semantic = ctx.semantic;
        let _source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = UnreachableChecker {
            diagnostics: Vec::new(),
        };
        checker.check_block(block);
        checker.diagnostics
    }
}

struct UnreachableChecker {
    diagnostics: Vec<LintDiagnostic>,
}

impl UnreachableChecker {
    fn check_block(&mut self, block: &Block) {
        let mut terminated = false;

        for stmt in &block.stmts {
            if terminated {
                let span = stmt.span();
                self.diagnostics.push(LintDiagnostic::new(
                    "unreachable_code",
                    "unreachable code".to_string(),
                    span,
                ));
                // Only the first unreachable statement per block is reported.
                break;
            }

            // Break is ordinarily a LastStatement, but Lua 5.2+ also allows it as a
            // regular Statement mid-block.
            if matches!(stmt, luck_ast::Statement::Break(_)) {
                terminated = true;
            }

            self.visit_statement(stmt);
        }
    }
}

impl Visitor for UnreachableChecker {
    fn visit_statement(&mut self, stmt: &luck_ast::Statement) {
        match stmt {
            luck_ast::Statement::DoBlock(d) => self.check_block(&d.block),
            luck_ast::Statement::WhileLoop(w) => self.check_block(&w.block),
            luck_ast::Statement::RepeatLoop(r) => self.check_block(&r.block),
            luck_ast::Statement::NumericFor(n) => self.check_block(&n.block),
            luck_ast::Statement::GenericFor(g) => self.check_block(&g.block),
            luck_ast::Statement::IfStatement(i) => {
                self.check_block(&i.block);
                for clause in &i.elseif_clauses {
                    self.check_block(&clause.block);
                }
                if let Some(else_clause) = &i.else_clause {
                    self.check_block(&else_clause.block);
                }
            }
            luck_ast::Statement::FunctionDecl(f) => self.check_block(&f.body.block),
            luck_ast::Statement::LocalFunction(f) => self.check_block(&f.body.block),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&UnreachableCode, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_code_after_break() {
        // On Lua 5.2+ break can sit mid-block, so a statement can follow it.
        let diags = run("while true do break print(1) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_only_first_unreachable_statement() {
        let diags = run("for i = 1, 3 do break print(1) print(2) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_break_as_last_statement() {
        let diags = run("while true do print(1) break end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_straight_line_code() {
        let diags = run("local x = 1\nprint(x)");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
