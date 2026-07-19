use luck_ast::Expression;
use luck_token::TokenKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

/// Lua's comparison operators (`<`, `<=`, `>`, `>=`, `==`, `~=`) are
/// all left-associative with the same precedence, so `a < b == c` parses
/// as `(a < b) == c`. The boolean result of the inner comparison is then
/// compared by value against `c`, almost never what was intended.
pub struct ComparisonPrecedence;

impl Rule for ComparisonPrecedence {
    fn name(&self) -> &'static str {
        "comparison_precedence"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "chained comparison parses by left-associativity, not as a math chain"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn is_compare_op(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Less
            | TokenKind::LessEqual
            | TokenKind::Greater
            | TokenKind::GreaterEqual
            | TokenKind::EqualEqual
            | TokenKind::TildeEqual
    )
}

/// Returns the inner comparison binop if `expr` is one and is not wrapped
/// in explicit parentheses (parentheses signal author intent).
fn inner_compare(expr: &Expression) -> Option<&luck_ast::expr::BinaryOp> {
    if let Expression::BinaryOp(binop) = expr
        && is_compare_op(&binop.op.kind)
    {
        return Some(binop);
    }
    None
}

/// A bare `not ...` operand. A `Parenthesized` wrapper around the `not`
/// means the author was explicit (`(not x) == y`), so it does not count.
fn is_unparenthesized_not(expr: &Expression) -> bool {
    matches!(expr, Expression::UnaryOp(unop) if matches!(unop.op.kind, TokenKind::Not))
}

impl NodeRule for ComparisonPrecedence {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::BinaryOp]);
        Some(&TYPES)
    }
    fn on_expression(&self, expr: &Expression, _ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::BinaryOp(outer) = expr
            && is_compare_op(&outer.op.kind)
        {
            let chained_side = if inner_compare(&outer.left).is_some() {
                Some("left")
            } else if inner_compare(&outer.right).is_some() {
                Some("right")
            } else {
                None
            };

            if let Some(side) = chained_side {
                out.push(LintDiagnostic::new("comparison_precedence", format!(
                        "chained comparison on the {side} side; this parses as `(a op b) op c`, not as a math-style chain"
                    ), outer.span).with_help(
                        "split into two boolean expressions joined with `and`, or parenthesize to make intent explicit"
                            .to_string(),
                    ));
            }

            if is_unparenthesized_not(&outer.left) && !is_unparenthesized_not(&outer.right) {
                let (parsed_as, help) = match &outer.op.kind {
                    TokenKind::EqualEqual => (
                        "(not x) == y",
                        "consider `x ~= y`, or add parentheses to make intent explicit",
                    ),
                    TokenKind::TildeEqual => (
                        "(not x) ~= y",
                        "consider `x == y`, or add parentheses to make intent explicit",
                    ),
                    TokenKind::Less => ("(not x) < y", "add parentheses to clarify intent"),
                    TokenKind::LessEqual => ("(not x) <= y", "add parentheses to clarify intent"),
                    TokenKind::Greater => ("(not x) > y", "add parentheses to clarify intent"),
                    TokenKind::GreaterEqual => {
                        ("(not x) >= y", "add parentheses to clarify intent")
                    }
                    _ => return,
                };
                out.push(
                    LintDiagnostic::new(
                        "comparison_precedence",
                        format!(
                            "`not` applies only to the left operand; this parses as `{parsed_as}`"
                        ),
                        outer.span,
                    )
                    .with_help(help.to_string()),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&ComparisonPrecedence, source, LuaVersion::Lua54)
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
        // `(a < b) == c`: the parens signal explicit author intent.
        let diags = run("local x = (a < b) == c");
        assert!(
            diags.is_empty(),
            "parenthesized intent should be respected: {diags:?}"
        );
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
        // `a < b and b < c`: proper math-chain idiom in Lua.
        let diags = run("local x = a < b and b < c");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
