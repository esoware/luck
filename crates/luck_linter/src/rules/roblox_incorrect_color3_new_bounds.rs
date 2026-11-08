use luck_ast::Expression;

use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::roblox::{global_new_call, literal_arg_value};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RobloxIncorrectColor3NewBounds;

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
        "`Color3.new` component outside the 0 to 1 range"
    }
}

impl NodeRule for RobloxIncorrectColor3NewBounds {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallExpr])
    }
    fn on_expression(
        &self,
        expr: &Expression,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let Some((call, args)) = global_new_call(expr, "Color3", context.semantic) else {
            return;
        };
        let has_out_of_bounds = args
            .iter()
            .filter_map(literal_arg_value)
            .any(|value| !(0.0f64..=1.0f64).contains(&value));
        if has_out_of_bounds {
            findings.push(
                Finding::new("`Color3.new` components are on a 0 to 1 scale", call.span)
                    .with_help("use `Color3.fromRGB` for 0 to 255 values"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{run_rule, run_rule_roblox};

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule_roblox(&RobloxIncorrectColor3NewBounds, source)
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
        let diags = run_rule(
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
