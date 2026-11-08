use luck_ast::Expression;

use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::roblox::{global_new_call, literal_arg_value};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RobloxSuspiciousUdim2New;

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
        "`UDim2.new` called with fewer components than expected"
    }
}

impl NodeRule for RobloxSuspiciousUdim2New {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallExpr])
    }
    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let Some((call, args)) = global_new_call(expr, "UDim2", context.semantic) else {
            return;
        };
        if args.is_empty() || args.len() >= 4 {
            return;
        }
        let values: Vec<f64> = args.iter().filter_map(literal_arg_value).collect();
        if values.len() != args.len() {
            return;
        }
        let is_scale_sized = values.iter().all(|value| (-1.0f64..=1.0f64).contains(value));
        let suggestion = if is_scale_sized { "UDim2.fromScale" } else { "UDim2.fromOffset" };
        findings.push(
            Finding::new(
                "`UDim2.new` takes 4 components (`xScale`, `xOffset`, `yScale`, `yOffset`)",
                call.span,
            )
            .with_help(format!("did you mean to use `{suggestion}`?")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{run_rule, run_rule_roblox};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule_roblox(&RobloxSuspiciousUdim2New, source)
    }

    #[test]
    fn flags_two_scale_args() {
        let diags = run("local u = UDim2.new(0.5, 1)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].help.as_deref().unwrap().contains("fromScale"), "{diags:?}");
    }

    #[test]
    fn flags_two_offset_args() {
        let diags = run("local u = UDim2.new(100, 200)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].help.as_deref().unwrap().contains("fromOffset"), "{diags:?}");
    }

    #[test]
    fn flags_single_negative_arg() {
        let diags = run("local u = UDim2.new(-0.5)");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].help.as_deref().unwrap().contains("fromScale"), "{diags:?}");
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
        let diags = run_rule(
            &RobloxSuspiciousUdim2New,
            "local u = UDim2.new(1, 2)",
            luck_token::LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
