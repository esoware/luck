use luck_ast::Expression;
use luck_token::{BinOp, UnOp};

use crate::diagnostic::{Category, Finding, Severity};
use crate::fix::{Fix, TextEdit};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnnecessaryNegation;

impl Rule for UnnecessaryNegation {
    fn name(&self) -> &'static str {
        "unnecessary_negation"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "negated comparison that could use the opposite operator"
    }
}

impl NodeRule for UnnecessaryNegation {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::UnaryOp])
    }
    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let Expression::UnaryOp(unary) = expr else {
            return;
        };
        if unary.op != UnOp::Not {
            return;
        }
        let Expression::Parenthesized(parenthesized) = &unary.operand else {
            return;
        };
        let Expression::BinaryOp(comparison) = &parenthesized.expr else {
            return;
        };
        let Some(opposite_op) = opposite_comparison(comparison.op) else {
            return;
        };
        let lhs = comparison.left.span().text(context.source);
        let rhs = comparison.right.span().text(context.source);
        let op_text = opposite_op.static_text();
        // Only `==` and `~=` get a fix, since `a ~= b` is defined as
        // `not (a == b)`. Flipping `<` or `<=` changes the result for NaN
        // and can call a different metamethod.
        let is_equality = matches!(comparison.op, BinOp::Eq | BinOp::Ne);
        let fix = is_equality.then(|| Fix {
            description: "invert the comparison operator".to_owned(),
            // Keep the parens. This node can't see its parent, and a bare
            // comparison could regroup with a neighboring `..`, arithmetic,
            // or comparison operator.
            edits: vec![TextEdit {
                span: unary.span,
                replacement: format!("({lhs} {op_text} {rhs})"),
            }],
        });
        findings.push(
            Finding::new(
                format!("negated comparison, consider `{lhs} {op_text} {rhs}`"),
                unary.span,
            )
            .with_optional_fix(fix),
        );
    }
}

/// The comparison that holds exactly when `op` doesn't, NaN aside.
const fn opposite_comparison(op: BinOp) -> Option<BinOp> {
    match op {
        BinOp::Eq => Some(BinOp::Ne),
        BinOp::Ne => Some(BinOp::Eq),
        BinOp::Lt => Some(BinOp::Ge),
        BinOp::Le => Some(BinOp::Gt),
        BinOp::Gt => Some(BinOp::Le),
        BinOp::Ge => Some(BinOp::Lt),
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::FloorDiv
        | BinOp::Mod
        | BinOp::Pow
        | BinOp::Concat
        | BinOp::And
        | BinOp::Or
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => None,
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&UnnecessaryNegation, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_negated_equality() {
        let diags = run("local r = not (a == b)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("a ~= b"), "{diags:?}");
        assert!(diags[0].fix.is_some(), "{diags:?}");
    }

    #[test]
    fn flags_negated_inequality() {
        let diags = run("local r = not (a ~= b)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_some(), "{diags:?}");
    }

    #[test]
    fn flags_negated_relational_without_fix() {
        let diags = run("local r = not (a < b)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("a >= b"), "{diags:?}");
        assert!(diags[0].fix.is_none(), "{diags:?}");
    }

    #[test]
    fn flags_negated_greater_equal_without_fix() {
        let diags = run("local r = not (a >= b)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("a < b"), "{diags:?}");
        assert!(diags[0].fix.is_none(), "{diags:?}");
    }

    #[test]
    fn ignores_negated_and() {
        let diags = run("local r = not (a and b)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_negated_arithmetic() {
        let diags = run("local r = not (a + b)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_unparenthesized_negation() {
        // `not a == b` parses as `(not a) == b`; `comparison_precedence`
        // owns that shape.
        let diags = run("local r = not a == b");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_and_fixes_negated_equality() {
        let source = "local r = not (a == b)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local r = (a ~= b)");
    }

    #[test]
    fn flags_and_fixes_negated_inequality() {
        let source = "local r = not (x.y ~= f(1))";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local r = (x.y == f(1))");
    }
}
