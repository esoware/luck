use luck_ast::Expression;
use luck_token::BinOp;

use crate::binary::operator_pairs;
use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConstantTableComparison;

impl_rule!(
    ConstantTableComparison,
    "constant_table_comparison",
    Correctness,
    Error,
    "comparison with a table literal, which compares by identity"
);

impl NodeRule for ConstantTableComparison {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::BinaryOp])
    }
    fn on_expr(&self, expr: &Expression, _context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let Expression::BinaryOp(run) = expr else {
            return;
        };
        if !run.links.first().op.is_comparison() {
            return;
        }
        let is_table =
            |operand: Option<&Expression>| matches!(operand, Some(Expression::TableConstructor(_)));
        for pair in operator_pairs(run) {
            if matches!(pair.op, BinOp::Eq | BinOp::Ne)
                && (is_table(pair.lhs) || is_table(pair.rhs))
            {
                findings.push(
                    Finding::new(
                        "tables compare by identity, so a table literal never equals another value",
                        pair.span(),
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
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
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
