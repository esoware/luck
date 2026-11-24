use luck_ast::{AstKind, AstType, ContentEq as _};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct IfSameThenElse;

declare_lint_rule!(
    IfSameThenElse,
    Suspicious,
    Warning,
    "`if` and `else` branches have identical bodies"
);

impl Rule for IfSameThenElse {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::IfStatement]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::IfStatement(if_stmt) = node.kind() else {
            return;
        };
        let Some(else_clause) = &if_stmt.else_clause else {
            return;
        };
        if !if_stmt.block.stmts.is_empty() && if_stmt.block.content_eq(&else_clause.block) {
            let finding =
                Finding::new("`if` and `else` branches have identical bodies", if_stmt.span);
            // An `elseif` keeps the first condition meaningful.
            context.diagnostic(if if_stmt.elseif_clauses.is_empty() {
                finding.with_help("the condition has no effect, so remove the `if`")
            } else {
                finding
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::IfSameThenElse, source, LuaVersion::Lua54)
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

    #[test]
    fn flags_branches_that_differ_only_in_spacing_and_comments() {
        let diags = run("\
if a then
  print(1, x) -- the first
else
  print( 1,x )
end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_empty_branches() {
        let diags = run("\
if a then
  -- nothing
else
  -- nothing
end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_identical_branches_with_elseif_without_dead_condition_help() {
        let diags = run("if a then f() elseif b then g() else f() end");
        assert!(
            diags.iter().all(|diag| !diag.help.as_deref().unwrap_or("").contains("no effect")),
            "the condition decides whether `elseif` runs, so removing the `if` isn't safe: {diags:?}"
        );
    }
}
