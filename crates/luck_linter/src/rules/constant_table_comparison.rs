use luck_ast::Expression;
use luck_token::BinOp;

use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConstantTableComparison;

impl Rule for ConstantTableComparison {
    fn name(&self) -> &'static str {
        "constant_table_comparison"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "comparison with a table literal, which compares by identity"
    }
}

impl NodeRule for ConstantTableComparison {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::BinaryOp])
    }
    fn on_expression(
        &self,
        expr: &Expression,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Expression::BinaryOp(binary) = expr
            && matches!(binary.op, BinOp::Eq | BinOp::Ne)
            && (matches!(&binary.left, Expression::TableConstructor(_))
                || matches!(&binary.right, Expression::TableConstructor(_)))
        {
            findings.push(
                Finding::new(
                    "tables compare by identity, so a table literal never equals another value",
                    binary.span,
                )
                .with_help("compare table contents instead"),
            );
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
        let diags = run("local t = {}\nlocal same = x == t");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
