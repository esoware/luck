use luck_ast::{AstKind, AstType, Call, GetSpan as _};
use luck_diagnostics::Tag;
use luck_semantic::{AstNode, AstTypesBitset};

use crate::call::{CallArgs, is_global_call};
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnnecessaryAssert;

declare_lint_rule!(
    UnnecessaryAssert,
    Suspicious,
    Warning,
    "`assert` on a constant value always passes or always errors"
);

impl Rule for UnnecessaryAssert {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        for (_, call) in chain.calls() {
            if let Some(finding) = check_call(call, context) {
                context.diagnostic(finding);
            }
        }
    }
}

fn check_call(call: Call<'_>, context: &LintContext<'_, '_>) -> Option<Finding> {
    if !is_global_call(call, "assert", context.scoping()) {
        return None;
    }
    let is_truthy = CallArgs::new(call.args).next()?.to_boolean(context)?;
    let finding = if is_truthy {
        Finding::new("`assert` on a constant value always passes", call.span())
            .with_tag(Tag::Unnecessary)
            .with_help("remove the `assert` or check a real condition")
    } else {
        Finding::new("`assert` on a constant falsy value always errors", call.span())
            .with_help("use `error()` to raise unconditionally")
    };
    Some(finding)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        // Its `format` comes through the string metatable, which a program can
        // change.
        ("assert(`a{x}`)", LuaTarget::Luau),
        (
            "\
local x = 1
assert(x)",
            LuaTarget::Lua54,
        ),
        ("assert(next({}))", LuaTarget::Lua54),
        (
            "\
local assert = function() end
assert(true)",
            LuaTarget::Lua54,
        ),
        ("assert()", LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("assert(true)", LuaTarget::Lua54),
        // 0 is truthy in Lua.
        ("assert(0)", LuaTarget::Lua54),
        (r#"assert("always")"#, LuaTarget::Lua54),
        ("assert({})", LuaTarget::Lua54),
        (r#"assert(false, "unreachable")"#, LuaTarget::Lua54),
        ("assert(nil)", LuaTarget::Lua54),
        ("assert(1 + 1 == 2)", LuaTarget::Lua54),
        ("assert(x or true)", LuaTarget::Lua54),
        ("assert(not 1)", LuaTarget::Lua54),
        (r#"assert"""#, LuaTarget::Lua54),
        ("assert{}", LuaTarget::Lua54),
        ("assert{ false }", LuaTarget::Lua54),
        ("local ok = assert(true)", LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnnecessaryAssert, PASS, FAIL).test_and_snapshot();
    }
}
