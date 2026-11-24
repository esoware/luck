use luck_ast::{AstKind, AstType};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::{BinOp, CompoundOp, Span};

use crate::binary::operator_pairs;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::number::has_number_value;
use crate::rule::{Rule, declare_lint_rule};

/// Float division by zero gives `inf` or `nan`, and from Lua 5.3 integer
/// `%` and `//` by zero raise an error. Either way a divisor that's always
/// zero is a mistake.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DivideByZero;

declare_lint_rule!(DivideByZero, Correctness, Warning, "division or modulo by a constant zero");

impl Rule for DivideByZero {
    const NODE_TYPES: AstTypesBitset =
        AstTypesBitset::from_types(&[AstType::BinaryOp, AstType::CompoundAssignment]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let kind = node.kind();
        // Luau
        if let AstKind::CompoundAssignment(compound) = kind {
            if let Some(op_name) = compound_divisive_op_name(compound.op)
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
        // Lua 5.3+, Luau
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
        | BinOp::Or
        | BinOp::BitAnd
        | BinOp::BitOr
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => None,
    }
}

const fn compound_divisive_op_name(op: CompoundOp) -> Option<&'static str> {
    match op {
        CompoundOp::Div => Some("division"),
        CompoundOp::Mod => Some("modulo"),
        CompoundOp::FloorDiv => Some("floor division"),
        CompoundOp::Add
        | CompoundOp::Sub
        | CompoundOp::Mul
        | CompoundOp::Pow
        | CompoundOp::Concat => None,
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;
    use crate::rules::RuleId;

    fn run(source: &str, version: LuaVersion) -> Vec<Message> {
        run_rule(RuleId::DivideByZero, source, version)
    }

    #[test]
    fn flags_division_by_zero() {
        let diags = run("local r = x / 0", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_modulo_by_zero() {
        let diags = run("local r = x % 0", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_floor_division_by_zero() {
        let diags = run("local r = x // 0", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_float_zero() {
        // Dividing by 0.0 gives `inf` or `nan`, which is still a bug.
        let diags = run("local r = x / 0.0", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_a_divisor_that_evaluates_to_zero() {
        for divisor in ["-0", "-0.0", "(1 - 1)", "(0 * 5)"] {
            let diags = run(&format!("local r = x / {divisor}"), LuaVersion::Lua54);
            assert_eq!(diags.len(), 1, "{divisor}: {diags:?}");
        }
    }

    #[test]
    fn ignores_nonzero_divisor() {
        let diags = run("local r = x / y", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_division_by_constant() {
        let diags = run("local r = x / 2", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_hex_zero() {
        let diags = run("local r = x / 0x0", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_compound_division_by_zero_on_luau() {
        let diags = run(
            "\
local x = 1
x /= 0",
            LuaVersion::Luau,
        );
        assert_eq!(diags.len(), 1, "`x /= 0` divides by zero: {diags:?}");
    }

    #[test]
    fn flags_binary_zero_divisor_on_luau() {
        let diags = run("local r = x / 0b0", LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "`0b0` is a literal zero: {diags:?}");
    }

    #[test]
    fn flags_zero_hex_float() {
        let diags = run("local r = x / 0x0p0", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_nonzero_hex_float() {
        let diags = run("local r = x / 0x1p4", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_compound_modulo_by_underscored_zero_on_luau() {
        let diags = run(
            "\
local x = 1
x %= 0_0",
            LuaVersion::Luau,
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_compound_multiplication_by_zero_on_luau() {
        let diags = run(
            "\
local x = 1
x *= 0",
            LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
