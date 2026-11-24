use luck_ast::{AstKind, AstType, GetSpan as _};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_stdlib::{ResolvedCallee, StdlibEntry};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MustUse;

declare_lint_rule!(
    MustUse,
    Suspicious,
    Warning,
    "return value of a must-use function is discarded"
);

/// A call statement drops the results of its chain's last call.
impl Rule for MustUse {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        let Some(index) = context.statement_call(node) else {
            return;
        };
        let Some(call) = chain.call_at(index) else {
            return;
        };
        if let Some(name) = context.callee(node, index).and_then(must_use_name) {
            context.diagnostic(
                Finding::new(format!("return value of `{name}` is discarded"), call.span())
                    .with_help("assign the result or remove the call"),
            );
        }
    }
}

fn must_use_name(callee: ResolvedCallee<'_>) -> Option<String> {
    let StdlibEntry::Function(function) = callee.entry() else {
        return None;
    };
    function.is_must_use.then(|| callee.name())
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        (
            "\
local s = tostring(1)
print(s)",
            LuaTarget::Lua54,
        ),
        // `table.insert` mutates, so discarding its result is fine.
        ("table.insert(t, 1)", LuaTarget::Lua54),
        ("obj:tostring()", LuaTarget::Lua54),
        (
            "\
local f = io.open('x')
f:seek('set')",
            LuaTarget::Lua54,
        ),
        // The call runs a full collection.
        ("collectgarbage()", LuaTarget::Luau),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("tostring(1)", LuaTarget::Lua54),
        // Discarding a pcall loses the success bool, which is the whole point of
        // calling it.
        ("pcall(f)", LuaTarget::Lua54),
        ("coroutine.create(f)", LuaTarget::Lua54),
        ("table.clone(t)", LuaTarget::Luau),
        (
            "\
local s = 'x'
s:upper()",
            LuaTarget::Lua54,
        ),
        ("tostring(42)", LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::MustUse, PASS, FAIL).test_and_snapshot();
    }
}
