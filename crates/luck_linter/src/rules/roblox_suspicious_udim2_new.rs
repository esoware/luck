use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_token::UnOp;
use luck_token::literal::{LuaNumber, parse_lua_number};
use luck_token::{StdlibEnvironment, TokenKind};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct RobloxSuspiciousUdim2New;

impl Rule for RobloxSuspiciousUdim2New {
    fn name(&self) -> &'static str {
        "roblox_suspicious_udim2_new"
    }

    fn category(&self) -> Category {
        Category::Suspicious
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "UDim2.new called with fewer components than expected."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn is_udim2_new_call(call: &FunctionCall, ctx: &LintContext) -> bool {
    if call.method.is_some() {
        return false;
    }
    let Expression::Var(var) = &call.callee else {
        return false;
    };
    let Var::FieldAccess(field) = var else {
        return false;
    };
    if !matches!(&field.name.kind, TokenKind::Identifier(name) if name == "new") {
        return false;
    }
    let Expression::Var(prefix_var) = &field.prefix else {
        return false;
    };
    let Var::Name(token) = prefix_var else {
        return false;
    };
    matches!(&token.kind, TokenKind::Identifier(name) if name == "UDim2")
        && !ctx.semantic.resolves_to_local("UDim2", token.span)
}

fn number_literal_value(literal: &luck_ast::expr::Literal) -> Option<f64> {
    match parse_lua_number(&literal.text, true)? {
        LuaNumber::Int(int_value) => Some(int_value as f64),
        LuaNumber::Float(float_value) => Some(float_value),
    }
}

fn literal_arg_value(expr: &Expression) -> Option<f64> {
    match expr {
        Expression::Number(literal) => number_literal_value(literal),
        Expression::UnaryOp(unary) if unary.op == UnOp::Neg => {
            if let Expression::Number(literal) = &unary.operand {
                number_literal_value(literal).map(|value| -value)
            } else {
                None
            }
        }
        _ => None,
    }
}

impl NodeRule for RobloxSuspiciousUdim2New {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::FunctionCallExpr]);
        Some(&TYPES)
    }
    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if ctx.semantic.environment != StdlibEnvironment::Roblox {
            return;
        }
        let Expression::FunctionCall(call) = expr else {
            return;
        };
        if !is_udim2_new_call(call, ctx) {
            return;
        }
        let FunctionArgs::Parenthesized { args, .. } = &call.args else {
            return;
        };
        if args.is_empty() || args.len() >= 4 {
            return;
        }
        let values: Vec<f64> = args.iter().filter_map(literal_arg_value).collect();
        if values.len() != args.len() {
            return;
        }
        let all_scale_sized = values.iter().all(|value| (-1.0..=1.0).contains(value));
        let suggestion = if all_scale_sized {
            "UDim2.fromScale"
        } else {
            "UDim2.fromOffset"
        };
        out.push(
            LintDiagnostic::new(
                self.name(),
                "UDim2.new takes 4 components (xScale, xOffset, yScale, yOffset)",
                call.span,
            )
            .with_help(format!("did you mean to use {suggestion}?")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule_roblox(&RobloxSuspiciousUdim2New, source)
    }

    #[test]
    fn flags_two_scale_args() {
        let diags = run("local u = UDim2.new(0.5, 1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].help.as_deref().unwrap().contains("fromScale"),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_two_offset_args() {
        let diags = run("local u = UDim2.new(100, 200)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].help.as_deref().unwrap().contains("fromOffset"),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_single_negative_arg() {
        let diags = run("local u = UDim2.new(-0.5)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].help.as_deref().unwrap().contains("fromScale"),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_three_args() {
        let diags = run("local u = UDim2.new(1, 0, 1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_four_args() {
        let diags = run("local u = UDim2.new(0.5, 0, 0.5, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_zero_args() {
        let diags = run("local u = UDim2.new()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_literal_args() {
        let diags = run("local x = 1 local u = UDim2.new(x, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_shadowed_local_udim2() {
        let diags = run("local UDim2 = {} local u = UDim2.new(1, 2)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_standalone_environment() {
        let diags = crate::test_support::run_rule(
            &RobloxSuspiciousUdim2New,
            "local u = UDim2.new(1, 2)",
            luck_token::LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
