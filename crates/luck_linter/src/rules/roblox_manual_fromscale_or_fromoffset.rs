use luck_ast::{AstKind, AstType, Call, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::call::calls_in;
use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::number::number_value;
use crate::roblox::global_new_call;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RobloxManualFromScaleOrFromOffset;

declare_lint_rule!(
    RobloxManualFromScaleOrFromOffset,
    Style,
    Warning,
    "`UDim2.new` that could be `UDim2.fromScale` or `UDim2.fromOffset`",
    fix = Fix
);

impl Rule for RobloxManualFromScaleOrFromOffset {
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
    let Some(args) = global_new_call(call, "UDim2", context.semantic) else {
        return;
    };
    let [x_scale_arg, x_offset_arg, y_scale_arg, y_offset_arg] = args else {
        return;
    };
    let (Some(x_scale), Some(x_offset), Some(y_scale), Some(y_offset)) = (
        number_value(x_scale_arg, context),
        number_value(x_offset_arg, context),
        number_value(y_scale_arg, context),
        number_value(y_offset_arg, context),
    ) else {
        return;
    };
    let is_scale_zero = x_scale == 0.0f64 && y_scale == 0.0f64;
    let is_offset_zero = x_offset == 0.0f64 && y_offset == 0.0f64;
    let (constructor, first_arg, second_arg) = if is_offset_zero && !is_scale_zero {
        ("fromScale", x_scale_arg, y_scale_arg)
    } else if is_scale_zero && !is_offset_zero {
        ("fromOffset", x_offset_arg, y_offset_arg)
    } else {
        return;
    };
    let replacement = [
        b"UDim2.",
        constructor.as_bytes(),
        b"(",
        first_arg.span().text(context.source),
        b", ",
        second_arg.span().text(context.source),
        b")",
    ]
    .concat();
    context.diagnostic(
        Finding::new(
            format!("this `UDim2.new` can be written as `UDim2.{constructor}`"),
            call.span(),
        )
        .with_help(format!("use `{}`", String::from_utf8_lossy(&replacement)))
        .with_fix(context, FixKind::Fix, |fixer| {
            fixer.replace(format!("replace with `UDim2.{constructor}`"), call.span(), replacement)
        }),
    );
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("local u = UDim2.new(0, 0, 0, 0)", LuaTarget::LuauRoblox),
        ("local u = UDim2.new(0.5, 100, 0.5, 0)", LuaTarget::LuauRoblox),
        ("local x = 0.5 local u = UDim2.new(x, 0, 1, 0)", LuaTarget::LuauRoblox),
        ("local u = UDim2.new(0.5, 0)", LuaTarget::LuauRoblox),
        ("local UDim2 = {} local u = UDim2.new(0.5, 0, 1, 0)", LuaTarget::LuauRoblox),
        ("local u = UDim2.new(0.5, 0, 1, 0)", LuaTarget::Luau),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("local u = UDim2.new(0.5, 0, 1, 0)", LuaTarget::LuauRoblox),
        ("local u = UDim2.new(0, 100, 0, 200)", LuaTarget::LuauRoblox),
        ("local u = UDim2.new(0, -100, 0, 0x20)", LuaTarget::LuauRoblox),
        // No fix, since it would delete the comment.
        ("local u = UDim2.new(0.5, --[[no offset]] 0, 1, 0)", LuaTarget::LuauRoblox),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RobloxManualFromScaleOrFromOffset, PASS, FAIL).test_and_snapshot();
    }
}
