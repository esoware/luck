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
        let mut checker = UnreachableChecker {
            diagnostics: Vec::new(),
        };
        checker.visit_block(ctx.block);
        checker.diagnostics
    }
}

struct UnreachableChecker {
    diagnostics: Vec<LintDiagnostic>,
}

impl<'ast> Visitor<'ast> for UnreachableChecker {
    fn visit_block(&mut self, block: &'ast Block) {
        // Break is ordinarily a LastStatement, but Lua 5.2+ also allows it as
        // a regular Statement mid-block, leaving whatever follows it in the
        // same block unreachable. Only the first such statement is reported.
        for pair in block.stmts.windows(2) {
            if matches!(pair[0], luck_ast::Statement::Break(_)) {
                self.diagnostics.push(LintDiagnostic::new(
                    "unreachable_code",
                    "unreachable code".to_string(),
                    pair[1].span(),
                ));
                break;
            }
        }
        self.walk_block(block);
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
