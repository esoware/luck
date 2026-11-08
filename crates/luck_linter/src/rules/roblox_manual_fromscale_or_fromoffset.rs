use luck_ast::{Expression, NodeType, NodeTypeSet};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::fix::{Fix, TextEdit};
use crate::roblox::{global_new_call, literal_arg_value};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RobloxManualFromScaleOrFromOffset;

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
        "`UDim2.new` that could be `UDim2.fromScale` or `UDim2.fromOffset`"
    }
}

impl NodeRule for RobloxManualFromScaleOrFromOffset {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallExpr])
    }
    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        let Some((call, args)) = global_new_call(expr, "UDim2", context.semantic) else {
            return;
        };
        let [x_scale_arg, x_offset_arg, y_scale_arg, y_offset_arg] = args.items.as_slice() else {
            return;
        };
        let values: Vec<f64> = args.iter().filter_map(literal_arg_value).collect();
        let [x_scale, x_offset, y_scale, y_offset] = values.as_slice() else {
            return;
        };
        let is_scale_zero = *x_scale == 0.0f64 && *y_scale == 0.0f64;
        let is_offset_zero = *x_offset == 0.0f64 && *y_offset == 0.0f64;
        let (constructor, first_arg, second_arg) = if is_offset_zero && !is_scale_zero {
            ("fromScale", x_scale_arg, y_scale_arg)
        } else if is_scale_zero && !is_offset_zero {
            ("fromOffset", x_offset_arg, y_offset_arg)
        } else {
            return;
        };
        let first_text = first_arg.span().text(context.source);
        let second_text = second_arg.span().text(context.source);
        let replacement = format!("UDim2.{constructor}({first_text}, {second_text})");
        diags.push(
            LintDiagnostic::new(
                self.name(),
                format!("this `UDim2.new` can be written as `UDim2.{constructor}`"),
                call.span,
            )
            .with_help(format!("use `{replacement}`"))
            .with_fix(Fix {
                description: format!("replace with `UDim2.{constructor}`"),
                edits: vec![TextEdit { span: call.span, replacement }],
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::{fix_and_reparse, run_rule, run_rule_roblox};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule_roblox(&RobloxManualFromScaleOrFromOffset, source)
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
    fn flags_and_fixes_to_fromscale() {
        let source = "local u = UDim2.new(0.5, 0, 1, 0)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
        assert_eq!(fixed, "local u = UDim2.fromScale(0.5, 1)");
    }

    #[test]
    fn flags_and_fixes_to_fromoffset_keeping_source_text() {
        let source = "local u = UDim2.new(0, -100, 0, 0x20)";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = fix_and_reparse(source, &diags, LuaVersion::Luau);
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
        let diags = run_rule(
            &RobloxManualFromScaleOrFromOffset,
            "local u = UDim2.new(0.5, 0, 1, 0)",
            LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
