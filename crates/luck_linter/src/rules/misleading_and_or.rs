use luck_ast::Expression;
use luck_token::BinOp;
use luck_value::ToBoolean as _;

use crate::binary::operator_pairs;
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
    fn on_expr(
        &self,
        expr: &Expression<'_>,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let Expression::BinaryOp(run) = expr else {
            return;
        };
        if run.links.first().op != BinOp::Or {
            return;
        }
        let version = context.semantic.version();
        for pair in operator_pairs(run) {
            if pair.op == BinOp::Or
                && let Some(lhs) = pair.lhs
                && let Expression::BinaryOp(inner) = lhs
                && inner.links.first().op == BinOp::And
                && inner.links.last().operand.to_boolean(&version) == Some(false)
            {
                findings.push(
                    Finding::new(
                        "`(x and FALSY) or y` always evaluates to `y`",
                        pair.span(context),
                    )
                    .with_help("use an `if` statement, or an if-then-else expression in Luau"),
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
    fn flags_a_middle_operand_that_is_always_falsy() {
        let diags = run("local r = (x and not 1) or y");
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
