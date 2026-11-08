use luck_ast::Expression;
use luck_token::BinOp;

use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

/// Flags `/`, `%`, and `//` with a literal zero divisor.
///
/// Float division by zero gives `inf` or `nan`, and from Lua 5.3 integer
/// `%` and `//` by zero raise an error. Either way a literal zero divisor is
/// a mistake.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DivideByZero;

impl Rule for DivideByZero {
    fn name(&self) -> &'static str {
        "divide_by_zero"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "literal division or modulo by zero"
    }
}

/// Whether `expr` is a number literal equal to zero, such as `0`, `0.0`,
/// `0x0`, or `0e0`.
fn is_zero_literal(expr: &Expression) -> bool {
    let Expression::Number(literal) = expr else {
        return false;
    };
    let text = literal.text.as_str();
    if let Ok(value) = text.parse::<f64>() {
        return value == 0.0;
    }
    // `f64::from_str` doesn't take hex.
    if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return rest
            .bytes()
            .all(|byte| byte == b'0' || byte == b'.' || byte == b'p' || byte == b'P');
    }
    false
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

impl NodeRule for DivideByZero {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::BinaryOp])
    }
    fn on_expression(
        &self,
        expr: &Expression,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Expression::BinaryOp(binary) = expr
            && let Some(op_name) = divisive_op_name(binary.op)
            && is_zero_literal(&binary.right)
        {
            findings.push(
                Finding::new(format!("{op_name} by literal zero"), binary.span).with_help(
                    "guard with an `if divisor ~= 0 then` check or use a nonzero constant",
                ),
            );
        }
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
}
