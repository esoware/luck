use luck_ast::{AstKind, AstType, Call, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::call::calls_in;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::number::number_value;
use crate::roblox::global_new_call;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RobloxSuspiciousUdim2New;

impl_rule!(
    RobloxSuspiciousUdim2New,
    "roblox_suspicious_udim2_new",
    Suspicious,
    Warning,
    "`UDim2.new` called with fewer components than expected"
);

impl RuleRunner for RobloxSuspiciousUdim2New {
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
    let Some(args) = global_new_call(call, "UDim2", context.semantic) else {
        return;
    };
    if args.is_empty() || args.len() >= 4 {
        return;
    }
    let values: Vec<f64> = args.iter().filter_map(|arg| number_value(arg, context)).collect();
    if values.len() != args.len() {
        return;
    }
    let is_scale_sized = values.iter().all(|v| (-1.0f64..=1.0f64).contains(v));
    let suggestion = if is_scale_sized { "UDim2.fromScale" } else { "UDim2.fromOffset" };
    context.diagnostic(
        Finding::new(
            "`UDim2.new` takes 4 components (`xScale`, `xOffset`, `yScale`, `yOffset`)",
            call.span(),
        )
        .with_help(format!("did you mean to use `{suggestion}`?")),
    );
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::{run_rule, run_rule_roblox};

    fn run(source: &str) -> Vec<Message> {
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
        let diags =
            run_rule(&RobloxSuspiciousUdim2New, "local u = UDim2.new(1, 2)", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
