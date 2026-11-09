use luck_ast::Expression;
use luck_token::BinOp;

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Flags the `(cond and a) or b` ternary idiom when `a` is a literal `false`
/// or `nil`, which makes it always evaluate to `b`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MisleadingAndOr;

impl_rule!(
    MisleadingAndOr,
    "misleading_and_or",
    Correctness,
    Warning,
    "`and`/`or` ternary with a falsy middle operand always returns the right-hand value"
);

impl NodeRule for MisleadingAndOr {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::BinaryOp])
    }
    fn on_expr(&self, expr: &Expression, _context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        if let Expression::BinaryOp(outer) = expr
            && outer.op == BinOp::Or
        {
            let lhs = unparenthesize(&outer.left);
            if let Expression::BinaryOp(inner) = lhs
                && inner.op == BinOp::And
                && is_falsy_literal(&inner.right)
            {
                findings.push(
                    Finding::new("`(x and FALSY) or y` always evaluates to `y`", outer.span)
                        .with_help("use an `if` statement, or an if-then-else expression in Luau"),
                );
            }
        }
    }
}

/// Strips one layer of parentheses, since people write the idiom both with
/// and without them.
fn unparenthesize(expr: &Expression) -> &Expression {
    if let Expression::Parenthesized(parenthesized) = expr {
        return &parenthesized.expr;
    }
    expr
}

fn is_falsy_literal(expr: &Expression) -> bool {
    matches!(unparenthesize(expr), Expression::Nil(_) | Expression::False(_))
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&MisleadingAndOr, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_nil_middle() {
        let diags = run("local r = (x and nil) or y");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_false_middle() {
        let diags = run("local r = (x and false) or y");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_unparenthesized_form() {
        // Lua precedence: `and` binds tighter than `or`, so this is the
        // same shape with no parens.
        let diags = run("local r = x and nil or y");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_zero_middle() {
        // `0` is truthy in Lua, so the ternary works as intended.
        let diags = run("local r = (x and 0) or y");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_normal_ternary() {
        let diags = run("local r = (x and y) or z");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_or_without_and() {
        let diags = run("local r = x or y");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
