use luck_ast::expr::{Expression, FunctionArgs, FunctionCall, Var};
use luck_token::UnOp;
use luck_token::literal::{LuaNumber, parse_lua_number};
use luck_token::{StdlibEnvironment, TokenKind};

use crate::diagnostic::{Category, Fix, LintDiagnostic, Severity, TextEdit};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct RobloxManualFromScaleOrFromOffset;

impl Rule for RobloxManualFromScaleOrFromOffset {
    fn name(&self) -> &'static str {
        "roblox_manual_fromscale_or_fromoffset"
    }

    fn category(&self) -> Category {
        Category::Style
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "UDim2.new that could be UDim2.fromScale or UDim2.fromOffset."
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

impl NodeRule for RobloxManualFromScaleOrFromOffset {
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
        if args.len() != 4 {
            return;
        }
        let values: Vec<f64> = args.iter().filter_map(literal_arg_value).collect();
        let [x_scale, x_offset, y_scale, y_offset] = values.as_slice() else {
            return;
        };
        let arg_source = |index: usize| {
            let span = args.get(index).expect("length checked above").span();
            &ctx.source[span.start as usize..span.end as usize]
        };
        let (constructor, first, second) =
            if *x_offset == 0.0 && *y_offset == 0.0 && (*x_scale != 0.0 || *y_scale != 0.0) {
                ("fromScale", arg_source(0), arg_source(2))
            } else if *x_scale == 0.0 && *y_scale == 0.0 && (*x_offset != 0.0 || *y_offset != 0.0) {
                ("fromOffset", arg_source(1), arg_source(3))
            } else {
                return;
            };
        let replacement = format!("UDim2.{constructor}({first}, {second})");
        out.push(
            LintDiagnostic::new(
                self.name(),
                format!("this UDim2.new can be written as UDim2.{constructor}"),
                call.span,
            )
            .with_help(format!("use `{replacement}`"))
            .with_fix(Fix {
                description: format!("replace with UDim2.{constructor}"),
                edits: vec![TextEdit {
                    span: call.span,
                    replacement,
                }],
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule_roblox(&RobloxManualFromScaleOrFromOffset, source)
    }

    fn apply_fix(source: &str, diags: &[LintDiagnostic]) -> String {
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fix = diags[0].fix.as_ref().expect("fix attached");
        let mut fixed = source.to_string();
        for edit in fix.edits.iter().rev() {
            fixed.replace_range(
                edit.span.start as usize..edit.span.end as usize,
                &edit.replacement,
            );
        }
        let parse = luck_parser::parse(&fixed, LuaVersion::Luau);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
        fixed
    }

    #[test]
    fn flags_scale_only_call() {
        let diags = run("local u = UDim2.new(0.5, 0, 1, 0)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("fromScale"), "{diags:?}");
    }

    #[test]
    fn flags_offset_only_call() {
        let diags = run("local u = UDim2.new(0, 100, 0, 200)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("fromOffset"), "{diags:?}");
    }

    #[test]
    fn fix_rewrites_to_fromscale() {
        let source = "local u = UDim2.new(0.5, 0, 1, 0)";
        let fixed = apply_fix(source, &run(source));
        assert_eq!(fixed, "local u = UDim2.fromScale(0.5, 1)");
    }

    #[test]
    fn fix_rewrites_to_fromoffset_keeping_source_text() {
        let source = "local u = UDim2.new(0, -100, 0, 0x20)";
        let fixed = apply_fix(source, &run(source));
        assert_eq!(fixed, "local u = UDim2.fromOffset(-100, 0x20)");
    }

    #[test]
    fn ignores_all_zero_call() {
        let diags = run("local u = UDim2.new(0, 0, 0, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_mixed_scale_and_offset() {
        let diags = run("local u = UDim2.new(0.5, 100, 0.5, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_literal_args() {
        let diags = run("local x = 0.5 local u = UDim2.new(x, 0, 1, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_short_calls() {
        let diags = run("local u = UDim2.new(0.5, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_shadowed_local_udim2() {
        let diags = run("local UDim2 = {} local u = UDim2.new(0.5, 0, 1, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_standalone_environment() {
        let diags = crate::test_support::run_rule(
            &RobloxManualFromScaleOrFromOffset,
            "local u = UDim2.new(0.5, 0, 1, 0)",
            LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
