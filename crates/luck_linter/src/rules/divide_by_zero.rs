use luck_ast::{Expression, Statement};
use luck_token::{BinOp, CompoundOp, Span};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::number::is_number_literal;
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Flags `/`, `%`, and `//` with a literal zero divisor.
///
/// Float division by zero gives `inf` or `nan`, and from Lua 5.3 integer
/// `%` and `//` by zero raise an error. Either way a literal zero divisor is
/// a mistake.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DivideByZero;

impl_rule!(
    DivideByZero,
    "divide_by_zero",
    Correctness,
    Warning,
    "literal division or modulo by zero"
);

impl NodeRule for DivideByZero {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::BinaryOp, NodeType::CompoundAssignment])
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        if let Expression::BinaryOp(binary) = expr
            && let Some(op_name) = divisive_op_name(binary.op)
            && is_number_literal(&binary.right, 0.0, context.semantic.version)
        {
            findings.push(zero_divisor_finding(op_name, binary.span));
        }
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        // Luau
        if let Statement::CompoundAssignment(compound) = stmt
            && let Some(op_name) = compound_divisive_op_name(compound.op)
            && is_number_literal(&compound.expr, 0.0, context.semantic.version)
        {
            findings.push(zero_divisor_finding(op_name, compound.span));
        }
    }
}

fn zero_divisor_finding(op_name: &str, span: Span) -> Finding {
    Finding::new(format!("{op_name} by literal zero"), span)
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

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        run_rule(&DivideByZero, source, version)
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
        let diags = run("local x = 1\nx /= 0", LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "`x /= 0` divides by a literal zero: {diags:?}");
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
        let diags = run("local x = 1\nx %= 0_0", LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_compound_multiplication_by_zero_on_luau() {
        let diags = run("local x = 1\nx *= 0", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
