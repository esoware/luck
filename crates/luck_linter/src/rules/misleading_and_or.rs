use luck_ast::{AstKind, AstType, Expression};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::BinOp;
use luck_value::ToBoolean as _;

use crate::binary::operator_pairs;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags the `(cond and a) or b` ternary idiom when `a` is a literal `false`
/// or `nil`, which makes it always evaluate to `b`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MisleadingAndOr;

declare_lint_rule!(
    MisleadingAndOr,
    Correctness,
    Warning,
    "`and`/`or` ternary with a falsy middle operand always returns the right-hand value"
);

impl Rule for MisleadingAndOr {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::BinaryOp]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::BinaryOp(run) = node.kind() else {
            return;
        };
        if run.links.first().op != BinOp::Or {
            return;
        }
        for pair in operator_pairs(run) {
            if pair.op == BinOp::Or
                && let Some(lhs) = pair.lhs
                && let Expression::BinaryOp(inner) = lhs
                && inner.links.first().op == BinOp::And
                && inner.links.last().operand.to_boolean(context) == Some(false)
            {
                context.diagnostic(
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
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        // `0` is truthy in Lua, so the ternary works as intended.
        "local r = (x and 0) or y",
        "local r = (x and y) or z",
        "local r = x or y",
    ];

    const FAIL: &[&str] = &[
        "local r = (x and nil) or y",
        "local r = (x and false) or y",
        "local r = (x and not 1) or y",
        // Lua precedence: `and` binds tighter than `or`, so this is the same shape
        // with no parens.
        "local r = x and nil or y",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::MisleadingAndOr, PASS, FAIL).test_and_snapshot();
    }
}
