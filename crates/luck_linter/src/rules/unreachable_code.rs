use luck_ast::Statement;

use crate::diagnostic::{Category, Finding, Severity};
use crate::rule::{LintContext, Rule, WholeRule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnreachableCode;

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
        "code after a mid-block `break` is unreachable"
    }
}

impl WholeRule for UnreachableCode {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        // Lua 5.2+ lets `break` sit mid-block, which leaves the rest of the
        // block unreachable. Only the first unreachable statement is reported.
        context
            .blocks
            .iter()
            .filter_map(|block| {
                block.stmts.iter().skip_while(|stmt| !matches!(stmt, Statement::Break(_))).nth(1)
            })
            .map(|stmt| Finding::new("unreachable code", stmt.span()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&UnreachableCode, source, LuaVersion::Lua54)
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
