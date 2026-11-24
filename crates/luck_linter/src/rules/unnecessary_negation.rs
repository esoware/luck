use core::ptr;

use luck_ast::{AstKind, AstType, BinaryOp, Expression, UnaryOp};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{BinOp, Span, UnOp};

use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnnecessaryNegation;

declare_lint_rule!(
    UnnecessaryNegation,
    Style,
    Warning,
    "negated comparison that could use the opposite operator",
    fix = Fix
);

impl Rule for UnnecessaryNegation {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::UnaryOp]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::UnaryOp(unary) = node.kind() else {
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
        context.diagnostic(
            Finding::new(
                format!(
                    "negated comparison, consider `{} {op_text} {}`",
                    String::from_utf8_lossy(lhs),
                    String::from_utf8_lossy(rhs)
                ),
                unary.span,
            )
            .with_fix(context, FixKind::Fix, |fixer| {
                let inverted = [lhs, b" ", op_text.as_bytes(), b" ", rhs].concat();
                let replacement = if needs_parentheses(node, unary, context) {
                    [&b"("[..], &inverted, b")"].concat()
                } else {
                    inverted
                };
                is_equality.then(|| {
                    fixer.replace("invert the comparison operator", unary.span, replacement)
                })
            }),
        );
    }
}

/// Whether a comparison written where `node`, the `not` `unary`, stands
/// would group with what's beside it.
///
/// It would as an operand of an operator that binds tighter than `and` and
/// `or`, of a unary operator, or of a cast, or as a chain's prefix.
/// An operand's neighbors are its run's operators, and a run is never
/// followed or preceded by one that binds tighter, or that one would have
/// taken the operand.
fn needs_parentheses(node: &AstNode<'_>, unary: &UnaryOp<'_>, context: &ContextHost<'_>) -> bool {
    if context.groupings.around(unary.span).is_some() {
        return false;
    }
    let nodes = context.nodes;
    let Some(parent) = nodes.parent_node(node.id()) else {
        return false;
    };
    let binds_tighter_than_and_or =
        |run: &BinaryOp<'_>| !matches!(run.links.first().op, BinOp::And | BinOp::Or);
    let kind = parent.kind();
    if let AstKind::BinaryOp(run) = kind {
        return binds_tighter_than_and_or(run);
    }
    if let AstKind::BinaryLink(_) = kind {
        return nodes
            .parent_kind(parent.id())
            .and_then(AstKind::as_binary_op)
            .is_some_and(binds_tighter_than_and_or);
    }
    if let AstKind::SuffixedExpression(chain) = kind {
        return matches!(
            &chain.prefix,
            Expression::UnaryOp(prefix) if ptr::eq(&raw const **prefix, unary)
        );
    }
    // Luau: a cast's operand.
    matches!(kind, AstKind::UnaryOp(_) | AstKind::TypeCast(_))
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

    use crate::diagnostic::Message;
    use crate::fixtures::{fix_and_reparse, run_rule};
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::UnnecessaryNegation, source, LuaVersion::Lua54)
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
        assert_eq!(fixed, "local r = a ~= b");
    }

    #[test]
    fn flags_and_fixes_negated_inequality() {
        let source = "local r = not (x.y ~= f(1))";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Lua54);
        assert_eq!(fixed, "local r = x.y == f(1)");
    }

    #[test]
    fn flags_and_fixes_with_parentheses_only_where_the_comparison_would_regroup() {
        for (source, expected) in [
            ("local r = (not (a == b))", "local r = (a ~= b)"),
            ("local r = not (not (a == b))", "local r = not (a ~= b)"),
            ("local r = not (a == (b == c))", "local r = a ~= (b == c)"),
            ("local r = not ((a == b) == c)", "local r = (a == b) ~= c"),
            ("local r = x and not (a == b)", "local r = x and a ~= b"),
            ("local r = not (a == b) == c", "local r = (a ~= b) == c"),
            ("local r = c == not (a == b)", "local r = c == (a ~= b)"),
            ("local r = x .. not (a == b)", "local r = x .. (a ~= b)"),
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
        assert_eq!(fixed, "local r = f(--[[why]] a) ~= b");
    }
}
