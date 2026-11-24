use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Luacheck 321: a local declared without a value is read where no assignment
/// can have reached it.
///
/// A name counts when its `local` gives it no value, as in `local x` or the
/// `b` of `local a, b = 1`. `local x = nil` is initialized, and
/// `redundant_nil_init` covers it instead. A field or index write through the
/// local is `mutating_uninitialized`'s (Luacheck 341).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AccessingUninitialized;

declare_lint_rule!(
    AccessingUninitialized,
    Correctness,
    Warning,
    "local is read before any value is assigned to it"
);

impl Rule for AccessingUninitialized {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let tree = context.scoping();
        for &(symbol_id, reference_id) in &context.local_flow().first_unassigned_uses {
            let first = tree.reference(reference_id);
            if context.is_field_write_prefix(first) {
                continue;
            }
            context.diagnostic(
                Finding::new(
                    format!(
                        "`{}` is read before any value is assigned, so it is `nil`",
                        tree.name(tree.symbol(symbol_id).name)
                    ),
                    first.span,
                )
                .with_help("initialize at declaration or assign before reading"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    fn unassigned_locals(count: usize) -> String {
        let mut source = String::new();
        for i in 0..count {
            writeln!(source, "local a{i}").unwrap();
        }
        source.push_str("print(a0)");
        source
    }

    const PASS: &[(&str, LuaTarget)] = &[
        // `redundant_nil_init` may flag this, but it counts as initialized.
        (
            "\
local x = nil
print(x)",
            LuaTarget::Lua54,
        ),
        (
            "\
local x
x = 1
print(x)",
            LuaTarget::Lua54,
        ),
        (
            "\
local x
if c then x = 1 end
print(x)",
            LuaTarget::Lua54,
        ),
        (
            "\
local x
x.foo = 1",
            LuaTarget::Lua54,
        ),
        (
            "\
local y
y[1] = 2",
            LuaTarget::Lua54,
        ),
        (
            "\
local _x
print(_x)",
            LuaTarget::Lua54,
        ),
        // The closure runs after the assignment.
        (
            "\
local helper
local function main() return helper() end
helper = function() return 1 end
return main()",
            LuaTarget::Lua54,
        ),
        // Later iterations read the written value.
        (
            "\
local last
for _, v in ipairs({1}) do if last then print(last) end last = v end",
            LuaTarget::Lua54,
        ),
        // The assignment runs before the read.
        (
            "\
local x
goto set
::use::
print(x)
do return end
::set::
x = 1
goto use",
            LuaTarget::Lua54,
        ),
        (
            "\
local x
local function set() x = 1 end
set()
print(x)",
            LuaTarget::Lua54,
        ),
        // The call can fill `b`.
        (
            "\
local a, b = f()
print(a, b)",
            LuaTarget::Lua54,
        ),
        // The `until` sees the body's locals.
        (
            "\
repeat local x
if c then x = 1 end until x",
            LuaTarget::Lua54,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        (
            "\
local x
print(x)",
            LuaTarget::Lua54,
        ),
        // `x.y.z = 1` reads `x.y` first, which isn't a write through `x`.
        (
            "\
local x
x.y.z = 1",
            LuaTarget::Lua54,
        ),
        // The right-hand side reads `nil`.
        (
            "\
local x
x = x + 1
return x",
            LuaTarget::Lua54,
        ),
        (
            "\
local x
if c then x = 1 else print(x) end",
            LuaTarget::Lua54,
        ),
        (
            "\
local x
print(x)
local function set() x = 1 end
set()",
            LuaTarget::Lua54,
        ),
        (
            "\
local a, b = 1
print(a, b)",
            LuaTarget::Lua54,
        ),
        // A fresh `x` starts each iteration.
        (
            "\
for _ = 1, 3 do local x
print(x)
x = 1 end",
            LuaTarget::Lua54,
        ),
    ];

    #[test]
    fn test() {
        let at_limit = unassigned_locals(200);
        // Only Luau's compiler, not luck, rejects this many active locals, and
        // walking each one's flow through all the others would be quadratic.
        let past_limit = unassigned_locals(201);
        let pass: Vec<_> =
            PASS.iter().copied().chain([(past_limit.as_str(), LuaTarget::Luau)]).collect();
        let fail: Vec<_> =
            FAIL.iter().copied().chain([(at_limit.as_str(), LuaTarget::Lua54)]).collect();
        Tester::new(RuleId::AccessingUninitialized, &pass, &fail).test_and_snapshot();
    }
}
