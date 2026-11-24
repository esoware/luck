use luck_ast::{AstKind, AstType, Expression, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{BinOp, UnOp};

use crate::binary::{OperatorPair, operator_pairs};
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Lua's comparison operators share one precedence and associate left, so
/// `a < b == c` parses as `(a < b) == c`. Unary `not` binds tighter than
/// any of them, so `not a == b` parses as `(not a) == b`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ComparisonPrecedence;

declare_lint_rule!(
    ComparisonPrecedence,
    Correctness,
    Error,
    "chained comparison or `not` operand that doesn't parse the way it reads"
);

impl Rule for ComparisonPrecedence {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::BinaryOp]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::BinaryOp(run) = node.kind() else {
            return;
        };
        if !run.links.first().op.is_comparison() {
            return;
        }
        for pair in operator_pairs(run) {
            check_pair(pair, context);
        }
    }
}

/// A comparison's operand that is itself a comparison is the run's
/// operators on that side, since a bare one would have joined the run.
fn check_pair(pair: OperatorPair<'_>, context: &LintContext<'_>) {
    let chained_side = if pair.lhs.is_none() {
        Some("left")
    } else if pair.rhs.is_none() {
        Some("right")
    } else {
        None
    };
    if let Some(side) = chained_side {
        context.diagnostic(
            Finding::new(
                format!(
                    "chained comparison on the {side} side parses as `(a op b) op c`, not as a math-style chain"
                ),
                pair.span(context),
            )
            .with_help(
                "split into two boolean expressions joined with `and`, or parenthesize to make intent explicit",
            ),
        );
    }

    // The author of `(not x) == y` was explicit.
    let is_unparenthesized_not = |operand: &Expression<'_>| {
        matches!(operand, Expression::UnaryOp(unary) if unary.op == UnOp::Not)
            && context.groupings.around(operand.span()).is_none()
    };
    if pair.lhs.is_some_and(is_unparenthesized_not) && !pair.rhs.is_some_and(is_unparenthesized_not)
    {
        let help = match pair.op {
            BinOp::Eq => "consider `x ~= y`, or add parentheses to make intent explicit",
            BinOp::Ne => "consider `x == y`, or add parentheses to make intent explicit",
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => "add parentheses to clarify intent",
            // Unreachable: the run's operators are all comparisons.
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
            | BinOp::Shr => return,
        };
        context.diagnostic(
            Finding::new(
                format!(
                    "`not` applies only to the left operand, so this parses as `(not x) {} y`",
                    pair.op.static_text()
                ),
                pair.span(context),
            )
            .with_help(help),
        );
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str) -> Vec<Message> {
        run_rule(RuleId::ComparisonPrecedence, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_less_then_equal() {
        let diags = run("local x = a < b == c");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_equal_then_equal() {
        let diags = run("local x = a == b == c");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_single_comparison() {
        let diags = run("local x = a < b");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_parenthesized_inner() {
        // The parentheses show the intent.
        let diags = run("local x = (a < b) == c");
        assert!(diags.is_empty(), "parenthesized intent should be respected: {diags:?}");
    }

    #[test]
    fn flags_less_chain() {
        let diags = run("local x = a < b < c");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_not_equality() {
        let diags = run("local x = not a == b");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("(not x) == y"), "{diags:?}");
    }

    #[test]
    fn flags_not_relational() {
        for op in ["<", "<=", ">", ">="] {
            let diags = run(&format!("local x = not a {op} b"));
            assert_eq!(diags.len(), 1, "op {op}: {diags:?}");
        }
    }

    #[test]
    fn ignores_parenthesized_not() {
        let diags = run("local x = (not a) == b");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_double_not() {
        // `not x == not y` compares two negations; Luau exempts this too.
        let diags = run("local x = not a == not b");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_logical_chain() {
        // The idiomatic way to write a range check in Lua.
        let diags = run("local x = a < b and b < c");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
