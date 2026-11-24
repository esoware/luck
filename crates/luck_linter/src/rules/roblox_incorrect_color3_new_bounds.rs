use luck_ast::{AstKind, AstType, Call, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::call::calls_in;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::number::number_value;
use crate::roblox::global_new_call;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RobloxIncorrectColor3NewBounds;

declare_lint_rule!(
    RobloxIncorrectColor3NewBounds,
    Correctness,
    Error,
    "`Color3.new` component outside the 0 to 1 range"
);

impl Rule for RobloxIncorrectColor3NewBounds {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

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
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("local c = Color3.new(0, 0.5, 1)", LuaTarget::LuauRoblox),
        ("local r = 255 local c = Color3.new(r, 0, 0)", LuaTarget::LuauRoblox),
        ("local Color3 = {} local c = Color3.new(255, 0, 0)", LuaTarget::LuauRoblox),
        ("local c = Color3.new(255, 0, 0)", LuaTarget::Luau),
        ("local c = Color3.fromRGB(255, 0, 0)", LuaTarget::LuauRoblox),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("local c = Color3.new(255, 0, 0)", LuaTarget::LuauRoblox),
        ("local c = Color3.new(- -2, 0, 0)", LuaTarget::LuauRoblox),
        ("local c = Color3.new(-1, 0, 0)", LuaTarget::LuauRoblox),
        ("local c = Color3.new(0.5, 1.5, 0)", LuaTarget::LuauRoblox),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RobloxIncorrectColor3NewBounds, PASS, FAIL).test_and_snapshot();
    }
}
