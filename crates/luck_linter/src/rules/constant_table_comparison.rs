use luck_ast::{AstKind, AstType, Expression};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::BinOp;

use crate::binary::operator_pairs;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ConstantTableComparison;

declare_lint_rule!(
    ConstantTableComparison,
    Correctness,
    Error,
    "comparison with a table literal, which compares by identity"
);

impl Rule for ConstantTableComparison {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::BinaryOp]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::BinaryOp(run) = node.kind() else {
            return;
        };
        if !run.links.first().op.is_comparison() {
            return;
        }
        let is_table = |operand: Option<&Expression<'_>>| {
            matches!(operand, Some(Expression::TableConstructor(_)))
        };
        for pair in operator_pairs(run) {
            if matches!(pair.op, BinOp::Eq | BinOp::Ne)
                && (is_table(pair.lhs) || is_table(pair.rhs))
            {
                context.diagnostic(
                    Finding::new(
                        "tables compare by identity, so a table literal never equals another value",
                        pair.span(context),
                    )
                    .with_help("compare table contents instead"),
                );
            }
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
        run_rule(RuleId::ConstantTableComparison, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_equality_with_table_literal() {
        let diags = run("local same = x == {}");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_inequality_with_table_literal_on_left() {
        let diags = run("local differs = {1} ~= x");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_comparison_of_table_variables() {
        let diags = run("\
local t = {}
local same = x == t");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
