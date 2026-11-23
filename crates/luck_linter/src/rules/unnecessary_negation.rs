use luck_ast::Expression;
use luck_token::{BinOp, Span, UnOp};

use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct UnnecessaryNegation;

impl_rule!(
    UnnecessaryNegation,
    "unnecessary_negation",
    Style,
    Warning,
    "negated comparison that could use the opposite operator",
    Fix
);

impl NodeRule for UnnecessaryNegation {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::UnaryOp])
    }
    fn on_expr(
        &self,
        expr: &Expression<'_>,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let Expression::UnaryOp(unary) = expr else {
            return;
        };
        if unary.op != UnOp::Not {
            return;
        }
        // A comparison binds looser than `not`, so one that's its operand is
        // in parentheses.
        let Expression::BinaryOp(comparison) = &unary.operand else {
            return;
        };
        // A run of comparisons groups from the left, so its last one applies
        // to the rest.
        let Some((last, before_last)) = comparison.links.split_last() else {
            return;
        };
        let Some(opposite_op) = opposite_comparison(last.op) else {
            return;
        };
        let lhs_operand = before_last.last().map_or(&comparison.first, |link| &link.operand);
        let lhs =
            Span::new(comparison.span.start, context.extent(lhs_operand).end).text(context.source);
        let rhs = context.extent(&last.operand).text(context.source);
        let op_text = opposite_op.static_text();
        // Only `==` and `~=` get a fix, since `a ~= b` is defined as
        // `not (a == b)`. Flipping `<` or `<=` changes the result for NaN
        // and can call a different metamethod.
        let is_equality = matches!(last.op, BinOp::Eq | BinOp::Ne);
        findings.push(
            Finding::new(
                format!("negated comparison, consider `{lhs} {op_text} {rhs}`"),
                unary.span,
            )
            .with_fix(context, FixKind::Fix, |fixer| {
                // This node can't see its parent, and a bare comparison could
                // regroup with a neighboring `..`, arithmetic, or comparison
                // operator, so it takes parentheses unless the `not` already
                // sits in some.
                let replacement = if context.groupings.around(unary.span).is_some() {
                    format!("{lhs} {op_text} {rhs}")
                } else {
                    format!("({lhs} {op_text} {rhs})")
                };
                is_equality.then(|| {
                    fixer.replace("invert the comparison operator", unary.span, replacement)
                })
            }),
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
    use crate::diagnostic::Message;
    use crate::fixtures::{fix_and_reparse, run_rule};

    fn run(source: &str) -> Vec<Message> {
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

    #[test]
    fn flags_and_fixes_keeping_one_pair_of_parentheses() {
        for (source, expected) in [
            ("local r = (not (a == b))", "local r = (a ~= b)"),
            ("local r = not (not (a == b))", "local r = not (a ~= b)"),
            ("local r = not (a == (b == c))", "local r = (a ~= (b == c))"),
            ("local r = not ((a == b) == c)", "local r = ((a == b) ~= c)"),
        ] {
            let diags = run(source);
            assert_eq!(diags.len(), 1, "{source}: {diags:?}");
            assert_eq!(fix_and_reparse(source, &diags, LuaVersion::Lua54), expected, "{source}");
        }
    }

    #[test]
    fn flags_without_fix_for_comment_beside_the_operator() {
        let diags = run("local r = not (a --[[why]] == b)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].fix.is_none(), "the fix would delete the comment: {diags:?}");
    }

    #[test]
    fn flags_and_fixes_keeping_comment_inside_an_operand() {
        let source = "local r = not (f(--[[why]] a) == b)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local r = (f(--[[why]] a) ~= b)");
    }
}
