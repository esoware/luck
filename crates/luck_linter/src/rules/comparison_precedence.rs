use luck_ast::Expression;
use luck_token::{BinOp, UnOp};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Flags chained comparisons like `a < b == c`, and a `not` on the left of
/// a comparison, as in `not a == b`.
///
/// Lua's comparison operators share one precedence and associate left, so
/// `a < b == c` parses as `(a < b) == c`. Unary `not` binds tighter than
/// any of them, so `not a == b` parses as `(not a) == b`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ComparisonPrecedence;

impl_rule!(
    ComparisonPrecedence,
    "comparison_precedence",
    Correctness,
    Error,
    "chained comparison or `not` operand that doesn't parse the way it reads"
);

impl NodeRule for ComparisonPrecedence {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::BinaryOp])
    }
    fn on_expr(&self, expr: &Expression, _context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let Expression::BinaryOp(outer) = expr else {
            return;
        };
        if !outer.op.is_comparison() {
            return;
        }

        let chained_side = if is_unparenthesized_comparison(&outer.left) {
            Some("left")
        } else if is_unparenthesized_comparison(&outer.right) {
            Some("right")
        } else {
            None
        };
        if let Some(side) = chained_side {
            findings.push(
                Finding::new(
                    format!(
                        "chained comparison on the {side} side parses as `(a op b) op c`, not as a math-style chain"
                    ),
                    outer.span,
                )
                .with_help(
                    "split into two boolean expressions joined with `and`, or parenthesize to make intent explicit",
                ),
            );
        }

        if is_unparenthesized_not(&outer.left) && !is_unparenthesized_not(&outer.right) {
            let help = match outer.op {
                BinOp::Eq => "consider `x ~= y`, or add parentheses to make intent explicit",
                BinOp::Ne => "consider `x == y`, or add parentheses to make intent explicit",
                BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                    "add parentheses to clarify intent"
                }
                // Unreachable: `outer.op` is a comparison.
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
            findings.push(
                Finding::new(
                    format!(
                        "`not` applies only to the left operand, so this parses as `(not x) {} y`",
                        outer.op.static_text()
                    ),
                    outer.span,
                )
                .with_help(help),
            );
        }
    }
}

/// Whether `expr` is a comparison with no parentheses around it. The author
/// of `(a < b) == c` was explicit.
fn is_unparenthesized_comparison(expr: &Expression) -> bool {
    matches!(expr, Expression::BinaryOp(binary) if binary.op.is_comparison())
}

/// Whether `expr` is a `not` with no parentheses around it. The author of
/// `(not x) == y` was explicit.
fn is_unparenthesized_not(expr: &Expression) -> bool {
    matches!(expr, Expression::UnaryOp(unary) if unary.op == UnOp::Not)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&ComparisonPrecedence, source, LuaVersion::Lua54)
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
