use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UndefinedVariable;

declare_lint_rule!(UndefinedVariable, Correctness, Error, "use of undefined variable");

impl Rule for UndefinedVariable {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let semantic = context.semantic;
        let tree = semantic.scoping();
        for reference in tree.global_references() {
            let name = tree.name(reference.name);
            // `setting_global` owns writes, and a global this file writes
            // (`function f() end`, `counter = 0`) counts as defined
            // everywhere in it, or script-style Lua would error on every use.
            if reference.kind.is_write()
                || semantic.is_defined_global(reference)
                || name == "_"
                || tree.is_assigned_global(reference.name)
            {
                continue;
            }
            context.diagnostic(
                Finding::new(format!("undefined variable `{name}`"), reference.span).with_help(
                    "declare it with `local`, or add it to `extra_globals` or a `std` definition",
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
global x = 1
return x",
            LuaTarget::Lua55,
        ),
        (
            "\
global function f() end
f()",
            LuaTarget::Lua55,
        ),
        (
            "\
global y
return y",
            LuaTarget::Lua55,
        ),
        (
            "\
global z <const> = 2
return z",
            LuaTarget::Lua55,
        ),
        (
            "\
print(counter)
counter = 0
function helper() end
helper()",
            LuaTarget::Lua54,
        ),
        (
            "\
fresh = 1
print(_)",
            LuaTarget::Lua54,
        ),
        ("counter += 1", LuaTarget::Luau),
        // `y` is a field of the local `_ENV`.
        (
            "\
local _ENV = { y = 1 }
return y",
            LuaTarget::Lua54,
        ),
        // `global *` declares every free name.
        (
            "\
global *
return y",
            LuaTarget::Lua55,
        ),
        // Type functions have the `types` library.
        (
            "\
type function t1(a)
 return types.string
end",
            LuaTarget::Luau,
        ),
        ("local function f(...) return arg end", LuaTarget::Lua51),
        // Lua 5.5 `...name` declares a binding; its uses aren't globals.
        (
            "\
local function f(...args) return args end
f(1)",
            LuaTarget::Lua55,
        ),
        ("print(tostring(1))", LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] =
        &[("print(missing)", LuaTarget::Lua54), ("print(undefined_var)", LuaTarget::Lua54)];

    #[test]
    fn test() {
        Tester::new(RuleId::UndefinedVariable, PASS, FAIL).test_and_snapshot();
    }
}
