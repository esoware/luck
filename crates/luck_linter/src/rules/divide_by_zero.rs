use luck_ast::Expression;
use luck_token::BinOp;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

/// Literal `/ 0`, `% 0`, and `// 0`. On Lua 5.1/5.2 these produce `inf`,
/// `nan`, or `nan` respectively (silently). On 5.3+ floor and modulo by
/// zero raise a runtime error and float division by zero still returns
/// `inf`/`nan`. Either way the literal-zero divisor is a programmer
/// mistake, not a defensible runtime choice.
pub struct DivideByZero;

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

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

/// Recognize literal zero: `0`, `0.0`, `0x0`, `0e0`, etc. We parse the
/// source slice as an `f64`; if it equals `0.0` (positive or negative
/// zero) the literal denotes zero. We deliberately also flag `0.0`
/// because IEEE division by float zero is `inf` or `nan`, not a useful
/// value.
fn is_zero_literal(expr: &Expression) -> bool {
    let Expression::Number(literal) = expr else {
        return false;
    };
    let text = &literal.text;
    if let Ok(value) = text.parse::<f64>() {
        return value == 0.0;
    }
    // Hex literals don't parse via `f64::from_str`; check manually.
    if let Some(rest) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return rest
            .bytes()
            .all(|byte| byte == b'0' || byte == b'.' || byte == b'p' || byte == b'P');
    }
    false
}

fn is_divisive_op(op: BinOp) -> bool {
    matches!(op, BinOp::Div | BinOp::Mod | BinOp::FloorDiv)
}

impl NodeRule for DivideByZero {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::BinaryOp]);
        Some(&TYPES)
    }
    fn on_expression(&self, expr: &Expression, _ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::BinaryOp(binop) = expr
            && is_divisive_op(binop.op)
            && is_zero_literal(&binop.right)
        {
            let op_name = match binop.op {
                BinOp::Div => "division",
                BinOp::Mod => "modulo",
                BinOp::FloorDiv => "floor division",
                // Unreachable per `is_divisive_op`.
                _ => "division",
            };
            out.push(
                LintDiagnostic::new(
                    "divide_by_zero",
                    format!("{op_name} by literal zero"),
                    binop.span,
                )
                .with_help(
                    "guard with an `if divisor ~= 0 then` check or use a nonzero constant"
                        .to_string(),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str, version: LuaVersion) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&DivideByZero, source, version)
    }

    #[test]
    fn flags_int_div_by_zero() {
        let diags = run("local r = x / 0", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_mod_by_zero() {
        let diags = run("local r = x % 0", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_floor_div_by_zero() {
        let diags = run("local r = x // 0", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_float_zero() {
        // Dividing by 0.0 returns inf/nan, still a bug.
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
