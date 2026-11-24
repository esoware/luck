use luck_ast::{AstKind, AstType, Call, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::call::calls_in;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::number::number_value;
use crate::roblox::global_new_call;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RobloxIncorrectColor3NewBounds;

impl_rule!(
    RobloxIncorrectColor3NewBounds,
    "roblox_incorrect_color3_new_bounds",
    Correctness,
    Error,
    "`Color3.new` component outside the 0 to 1 range"
);

impl RuleRunner for RobloxIncorrectColor3NewBounds {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[AstType::SuffixedExpression])
    }

    fn should_run(&self, host: &ContextHost<'_>) -> bool {
        host.semantic.target().is_roblox()
    }

    /// A call statement's value goes unused, so it isn't checked.
    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        let last = if context.is_statement(node) { chain.suffixes.len() } else { usize::MAX };
        for (index, call) in calls_in(chain) {
            if index.saturating_add(1) != last {
                check_call(call, context);
            }
        }
    }
}

fn check_call(call: Call<'_>, context: &LintContext<'_>) {
    let Some(args) = global_new_call(call, "Color3", context.semantic) else {
        return;
    };
    let has_out_of_bounds = args
        .iter()
        .filter_map(|arg| number_value(arg, context))
        .any(|v| !(0.0f64..=1.0f64).contains(&v));
    if has_out_of_bounds {
        context.diagnostic(
            Finding::new("`Color3.new` components are on a 0 to 1 scale", call.span())
                .with_help("use `Color3.fromRGB` for 0 to 255 values"),
        );
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::{run_rule, run_rule_roblox};

    fn run(source: &str) -> Vec<Message> {
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
