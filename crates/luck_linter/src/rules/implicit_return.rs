use luck_ast::{AstKind, AstType};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::cfg::Exit;
use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::function_node::FunctionNode;
use crate::rule::{Rule, declare_lint_rule};

/// A function that never returns a value is a procedure and isn't flagged,
/// so a bare `return` used as a guard clause doesn't count.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ImplicitReturn;

// "Return a value or nil" is idiomatic Lua, and neither Selene nor
// Luacheck flags it, so this stays off by default.
declare_lint_rule!(
    ImplicitReturn,
    Suspicious,
    Warning,
    "function returns on some paths but falls through on others"
);

/// A function body's block runs in the function's scope, whose graph it
/// asks for.
impl Rule for ImplicitReturn {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::Block]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        if !matches!(node.kind(), AstKind::Block(_)) {
            return;
        }
        let Some(function) = context
            .nodes
            .parent_node(node.id())
            .and_then(|body| FunctionNode::of(body, context.nodes))
        else {
            return;
        };
        if let Some(graph) = context.cfg(node.scope())
            && graph.reaches(Exit::ReturnValue)
            && graph.reaches(Exit::End)
        {
            context.diagnostic(
                Finding::new(
                    "function returns on some paths but falls through on others",
                    function.span,
                )
                .with_help(
                    "add a `return` on the fallthrough path to make the implicit `nil` explicit",
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("function f(x) if x then return 1 else return 2 end end", LuaTarget::Lua54),
        ("function f() print() end", LuaTarget::Lua54),
        // A bare `return` at the end is explicit, so nothing falls through.
        ("function f() return end", LuaTarget::Lua54),
        ("type T = number", LuaTarget::Luau),
        // `while true do ... return end` always returns.
        ("function f() while true do return 1 end end", LuaTarget::Lua54),
        // `continue` ends the repeat, so `return 2` runs.
        (
            "\
local function f(x)
 if x then return 1 end
 repeat continue until true
 return 2
end",
            LuaTarget::Luau,
        ),
        // `1` is always truthy, so nothing falls through.
        ("local function f(x) while 1 do if x then return 1 end end end", LuaTarget::Lua54),
        // No path returns a value, so no `nil` is implicit.
        ("local function f(x) if not x then return end print(x) end", LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("function f(x) if x then return 1 end end", LuaTarget::Lua54),
        ("local function f(x) if x then return 1 end end", LuaTarget::Lua54),
        ("local f = function(x) if x then return 1 end end", LuaTarget::Lua54),
        ("global function f(x) if x then return 1 end end", LuaTarget::Lua55),
        ("type function t1(a) if a then return a end end", LuaTarget::Luau),
        // The outer function never returns, so only the inner one fires.
        ("function f() local function g(x) if x then return 1 end end end", LuaTarget::Lua54),
        ("function f(x, y) if x then return 1 elseif y then return 2 end end", LuaTarget::Lua54),
        // The loop body may never run, so the function can fall through.
        ("function f() for i = 1, 10 do return i end end", LuaTarget::Lua54),
        // The `break` path falls off the end.
        (
            "local function f(c, d) if c then return 1 end while true do if d then break end end end",
            LuaTarget::Lua54,
        ),
        (
            "local function f(x, y) if x then return end if y then return 1 end end",
            LuaTarget::Lua54,
        ),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::ImplicitReturn, PASS, FAIL).test_and_snapshot();
    }
}
