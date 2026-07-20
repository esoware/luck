use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_token::UnOp;
use luck_token::literal::{LuaNumber, NumberSubtypes, parse_lua_number};
use luck_token::{StdlibEnvironment, TokenKind};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct RobloxIncorrectColor3NewBounds;

impl Rule for RobloxIncorrectColor3NewBounds {
    fn name(&self) -> &'static str {
        "roblox_incorrect_color3_new_bounds"
    }

    fn category(&self) -> Category {
        Category::Correctness
    }

    fn default_severity(&self) -> Severity {
        Severity::Error
    }

    fn description(&self) -> &'static str {
        "Color3.new component outside the 0 to 1 range."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn is_global_new_call(call: &FunctionCall, global: &str, ctx: &LintContext) -> bool {
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
    matches!(&token.kind, TokenKind::Identifier(name) if name == global)
        && !ctx.semantic.resolves_to_local(global, token.span)
}

fn number_literal_value(literal: &luck_ast::expr::Literal) -> Option<f64> {
    match parse_lua_number(&literal.text, NumberSubtypes::IntFloat)? {
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

impl NodeRule for RobloxIncorrectColor3NewBounds {
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
        if !is_global_new_call(call, "Color3", ctx) {
            return;
        }
        let FunctionArgs::Parenthesized { args, .. } = &call.args else {
            return;
        };
        let has_out_of_bounds = args
            .iter()
            .filter_map(literal_arg_value)
            .any(|value| !(0.0..=1.0).contains(&value));
        if has_out_of_bounds {
            out.push(
                LintDiagnostic::new(
                    self.name(),
                    "Color3.new components are on a 0 to 1 scale",
                    call.span,
                )
                .with_help("use Color3.fromRGB for 0 to 255 values".to_string()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule_roblox(&RobloxIncorrectColor3NewBounds, source)
    }

    #[test]
    fn flags_component_above_one() {
        let diags = run("local c = Color3.new(255, 0, 0)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_negative_component() {
        let diags = run("local c = Color3.new(-1, 0, 0)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_float_component_above_one() {
        let diags = run("local c = Color3.new(0.5, 1.5, 0)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_components_in_bounds() {
        let diags = run("local c = Color3.new(0, 0.5, 1)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_literal_args() {
        let diags = run("local r = 255 local c = Color3.new(r, 0, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_shadowed_local_color3() {
        let diags = run("local Color3 = {} local c = Color3.new(255, 0, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_standalone_environment() {
        let diags = crate::test_support::run_rule(
            &RobloxIncorrectColor3NewBounds,
            "local c = Color3.new(255, 0, 0)",
            luck_token::LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_other_constructors() {
        let diags = run("local c = Color3.fromRGB(255, 0, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
