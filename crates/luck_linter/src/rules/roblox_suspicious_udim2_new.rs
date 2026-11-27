use luck_ast::{AstType, Call, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::call::global_new_call;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::number::number_value;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RobloxSuspiciousUdim2New;

declare_lint_rule!(
    RobloxSuspiciousUdim2New,
    Suspicious,
    Warning,
    "`UDim2.new` called with fewer components than expected"
);

impl Rule for RobloxSuspiciousUdim2New {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn should_run(&self, host: &ContextHost<'_>) -> bool {
        host.semantic.stdlib().has_roblox
    }

    /// A call statement's value goes unused, so it isn't checked.
    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        for call in context.used_calls(node) {
            check_call(call, context);
        }
    }
}

fn check_call(call: Call<'_>, context: &LintContext<'_, '_>) {
    let Some(args) = global_new_call(call, "UDim2", context.scoping()) else {
        return;
    };
    if args.is_empty() || args.len() >= 4 {
        return;
    }
    let mut is_scale_sized = true;
    for arg in args {
        let Some(value) = number_value(arg, context) else {
            return;
        };
        is_scale_sized &= (-1.0..=1.0).contains(&value);
    }
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
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &["local u = UDim2.new(1, 2)"];

    const ROBLOX_PASS: &[&str] = &[
        "local u = UDim2.new(0.5, 0, 0.5, 0)",
        "local u = UDim2.new()",
        "local x = 1 local u = UDim2.new(x, 0)",
        "local UDim2 = {} local u = UDim2.new(1, 2)",
        "UDim2 = {} local u = UDim2.new(1, 2)",
    ];

    const ROBLOX_FAIL: &[&str] = &[
        "local u = UDim2.new(0.5, 1)",
        "local u = UDim2.new(100, 200)",
        "local u = UDim2.new(-0.5)",
        "local u = UDim2.new(1, 0, 1)",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RobloxSuspiciousUdim2New, PASS, &[])
            .with_roblox(ROBLOX_PASS, ROBLOX_FAIL)
            .test_and_snapshot();
    }
}
