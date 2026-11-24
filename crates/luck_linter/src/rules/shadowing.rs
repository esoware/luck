use luck_semantic::SymbolKind;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Shadowing;

declare_lint_rule!(
    Shadowing,
    Style,
    Warning,
    "variable shadows an outer variable with the same name"
);

impl Rule for Shadowing {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let tree = context.scoping();

        for symbol in tree.symbols() {
            let Some(shadowed_id) = symbol.shadows else {
                continue;
            };
            // A leading `_` marks a name as deliberately unused, and an
            // implicit `self` has no name the author could change. Lua 5.1's
            // `local arg = {...}` is how a vararg function replaces its
            // implicit `arg`.
            let name = tree.name(symbol.name);
            if name.starts_with('_')
                || symbol.kind.is_implicit()
                || tree.symbol(shadowed_id).kind == SymbolKind::ImplicitArg
            {
                continue;
            }
            context.diagnostic(
                Finding::new(
                    format!("variable `{name}` shadows outer variable"),
                    symbol.definition_span,
                )
                .with_label(
                    tree.symbol(shadowed_id).definition_span,
                    "outer variable defined here",
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
        (
            "\
local x = 1
do local y = 2 print(y) end",
            LuaTarget::Lua54,
        ),
        (
            "\
local _ = 1
do local _ = 2 end",
            LuaTarget::Lua54,
        ),
        // An implicit `self` has no name to change.
        (
            "\
local M = {}
function M:a()
 local t = {}
 function t:b() return self end
 return t
end
return M",
            LuaTarget::Lua54,
        ),
        ("local function f(...) local arg = { ... } return arg end", LuaTarget::Lua51),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        // Inner-scope redeclaration of an outer local. `redefining_local` covers
        // only the same block.
        (
            "\
local x = 1
do local x = 2 print(x) end",
            LuaTarget::Lua54,
        ),
        (
            "\
local i = 1
for i = 1, 3 do print(i) end",
            LuaTarget::Lua54,
        ),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::Shadowing, PASS, FAIL).test_and_snapshot();
    }
}
