use luck_ast::Statement;

use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnreachableCode;

impl_rule!(
    UnreachableCode,
    "unreachable_code",
    Suspicious,
    Warning,
    "code after a mid-block `break` is unreachable"
);

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
    fn flags_only_first_unreachable_stmt() {
        let diags = run("for i = 1, 3 do break print(1) print(2) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_break_as_last_stmt() {
        let diags = run("while true do print(1) break end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_straight_line_code() {
        let diags = run("local x = 1\nprint(x)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn bug_ignores_semicolon_after_break() {
        let diags = run("while true do break; end");
        assert!(diags.is_empty(), "a `;` separator isn't code: {diags:?}");
    }

    #[test]
    fn bug_ignores_label_after_break() {
        let diags = run("while c do if x then goto a end break ::a:: print(1) end");
        assert!(diags.is_empty(), "the `goto` reaches the label after `break`: {diags:?}");
    }
}
