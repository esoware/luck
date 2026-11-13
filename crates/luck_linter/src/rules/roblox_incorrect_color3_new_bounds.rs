use luck_ast::Call;

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::number::signed_number_value;
use crate::roblox::global_new_call;
use crate::rule::{CallPosition, LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RobloxIncorrectColor3NewBounds;

impl_rule!(
    RobloxIncorrectColor3NewBounds,
    "roblox_incorrect_color3_new_bounds",
    Correctness,
    Error,
    "`Color3.new` component outside the 0 to 1 range"
);

impl NodeRule for RobloxIncorrectColor3NewBounds {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::FunctionCallExpr])
    }
    fn on_call(
        &self,
        call: Call<'_>,
        position: CallPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if position == CallPosition::Statement {
            return;
        }
        let Some(args) = global_new_call(call, "Color3", context.semantic) else {
            return;
        };
        let has_out_of_bounds = args
            .iter()
            .filter_map(|arg| signed_number_value(arg, context.semantic.version))
            .any(|v| !(0.0f64..=1.0f64).contains(&v));
        if has_out_of_bounds {
            findings.push(
                Finding::new("`Color3.new` components are on a 0 to 1 scale", call.span())
                    .with_help("use `Color3.fromRGB` for 0 to 255 values"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

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
    fn flags_component_under_repeated_minus() {
        let diags = run("local c = Color3.new(- -2, 0, 0)");
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
            LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_other_constructors() {
        let diags = run("local c = Color3.fromRGB(255, 0, 0)");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
