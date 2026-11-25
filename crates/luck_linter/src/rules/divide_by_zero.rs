use luck_ast::{AstKind, AstType};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{BinOp, Span};

use crate::binary::operator_pairs;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::number::has_number_value;
use crate::rule::{Rule, declare_lint_rule};

/// Division by zero gives `inf` or `nan`, so a divisor that's always zero is
/// a mistake.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DivideByZero;

declare_lint_rule!(DivideByZero, Correctness, Warning, "division or modulo by a constant zero");

impl Rule for DivideByZero {
    const NODE_TYPES: AstTypesBitset =
        AstTypesBitset::from_types(&[AstType::BinaryOp, AstType::CompoundAssignment]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let kind = node.kind();
        if let AstKind::CompoundAssignment(compound) = kind {
            if let Some(op_name) = divisive_op_name(compound.op.binary_op())
                && has_number_value(&compound.expr, context, 0.0)
            {
                context.diagnostic(zero_divisor_finding(op_name, compound.span));
            }
            return;
        }
        let AstKind::BinaryOp(run) = kind else {
            return;
        };
        if run.precedence() != BinOp::Div.precedence() {
            return;
        }
        for pair in operator_pairs(run) {
            if let Some(op_name) = divisive_op_name(pair.op)
                && let Some(divisor) = pair.rhs
                && has_number_value(divisor, context, 0.0)
            {
                context.diagnostic(zero_divisor_finding(op_name, pair.span(context)));
            }
        }
    }
}

fn zero_divisor_finding(op_name: &str, span: Span) -> Finding {
    Finding::new(format!("{op_name} by zero"), span)
        .with_help("guard with an `if divisor ~= 0 then` check or use a nonzero constant")
}

const fn divisive_op_name(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Div => Some("division"),
        BinOp::Mod => Some("modulo"),
        BinOp::FloorDiv => Some("floor division"),
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Pow
        | BinOp::Concat
        | BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &[
        "local r = x / y",
        "local r = x / 2",
        "local r = x / 0x10",
        "\
local x = 1
x *= 0",
    ];

    const FAIL: &[&str] = &[
        "local r = x / 0",
        "local r = x % 0",
        "local r = x // 0",
        "local r = x / 0.0",
        "local r = x / -0",
        "local r = x / -0.0",
        "local r = x / (1 - 1)",
        "local r = x / (0 * 5)",
        "local r = x / 0x0",
        // `x /= 0` divides by zero.
        "\
local x = 1
x /= 0",
        // `0b0` is a literal zero.
        "local r = x / 0b0",
        "\
local x = 1
x %= 0_0",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::DivideByZero, PASS, FAIL).test_and_snapshot();
    }
}
