use luck_ast::{AstKind, AstType, Expression};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::BinOp;

use crate::binary::operator_pairs;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConstantTableComparison;

impl_rule!(
    ConstantTableComparison,
    "constant_table_comparison",
    Correctness,
    Error,
    "comparison with a table literal, which compares by identity"
);

impl RuleRunner for ConstantTableComparison {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[AstType::BinaryOp])
    }

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

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
        run_rule(&ConstantTableComparison, source, LuaVersion::Lua54)
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
