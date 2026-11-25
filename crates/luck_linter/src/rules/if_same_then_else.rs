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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::IfStatement(if_stmt) = node.kind() else {
            return;
        };
        // A name the condition binds reads differently in each branch.
        let (Some(else_clause), None) = (&if_stmt.else_clause, &if_stmt.binding) else {
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
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "if a then print(1) else print(2) end",
        "if a then print(1) end",
        "if local x = f() then print(x) else print(x) end",
        "\
if a then
  -- nothing
else
  -- nothing
end",
    ];

    const FAIL: &[&str] = &[
        "if a then print(1) else print(1) end",
        "if cond then x = 1 else x = 1 end",
        "\
if a then
  print(1, x) -- the first
else
  print( 1,x )
end",
        // The condition decides whether `elseif` runs, so removing the `if` isn't
        // safe.
        "if a then f() elseif b then g() else f() end",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::IfSameThenElse, PASS, FAIL).test_and_snapshot();
    }
}
