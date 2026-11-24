use luck_diagnostics::Tag;
use luck_semantic::SymbolKind;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::fix::FixKind;
use crate::rule::{Rule, declare_lint_rule};
use crate::underscore_fix::prefix_underscore;

/// Flags a function parameter that's never referenced (Luacheck 212).
///
/// Callbacks and methods often ignore parameters their interface requires,
/// so this has its own toggle, separate from `unused_variable`, which skips
/// parameters.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UnusedArgument;

declare_lint_rule!(
    UnusedArgument,
    Style,
    Warning,
    "function parameter is declared but never referenced",
    fix = Suggestion
);

impl Rule for UnusedArgument {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let semantic = context.semantic;
        let tree = semantic.scoping();

        for symbol in tree.symbols() {
            if symbol.kind != SymbolKind::Parameter {
                continue;
            }
            let name = tree.name(symbol.name);
            // A parameter spelled `self` is a receiver the interface requires.
            if name == "self"
                || name.starts_with('_')
                || !tree.resolved_references(symbol.id).is_empty()
            {
                continue;
            }

            context.diagnostic(
                Finding::new(format!("unused argument `{name}`"), symbol.definition_span)
                    .with_tag(Tag::Unnecessary)
                    .with_help("prefix with `_` to suppress this warning")
                    .with_fix(context, FixKind::Suggestion, |fixer| {
                        prefix_underscore(fixer, semantic, symbol)
                    }),
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
        ("local function f(...args) return args end", LuaTarget::Lua55),
        ("local function f(_x) end", LuaTarget::Lua54),
        ("local function f(x) return x end", LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        // Lua 5.5 named vararg binds like a parameter.
        ("local function f(...args) end", LuaTarget::Lua55),
        ("local function f(x) end", LuaTarget::Lua54),
        // Only `x` is flagged. `self` is the implicit method receiver.
        (
            "\
local obj = {}
function obj:method(x) end",
            LuaTarget::Lua54,
        ),
        ("local function f(a, b, c) return a + c end", LuaTarget::Lua54),
        // The rename shouldn't make `return _a` read the second parameter.
        ("local function f(_a, a) return _a end", LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::UnusedArgument, PASS, FAIL).test_and_snapshot();
    }
}
