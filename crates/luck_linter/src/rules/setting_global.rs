use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SettingGlobal;

declare_lint_rule!(
    SettingGlobal,
    Correctness,
    Warning,
    "implicit global variable assignment (missing `local`?)"
);

impl Rule for SettingGlobal {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let semantic = context.semantic;
        let tree = semantic.scoping();

        for reference in tree.global_references() {
            if !reference.kind.is_write() {
                continue;
            }
            // Writing a known global like `print` isn't a forgotten `local`.
            if semantic.is_defined_global(reference) {
                continue;
            }
            let name = tree.name(reference.name);
            // `_` is a conventional discard name, not an accidental global.
            if name == "_" {
                continue;
            }

            context.diagnostic(
                Finding::new(
                    format!("setting global variable `{name}` (missing `local`?)"),
                    reference.span,
                )
                .with_help("add `local` before the first assignment"),
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
        ("global x = 1", LuaTarget::Lua55),
        (
            "\
global x
x = 1",
            LuaTarget::Lua55,
        ),
        (
            "\
global function f() end
f = nil",
            LuaTarget::Lua55,
        ),
        (
            "\
print = print
_ = 1",
            LuaTarget::Lua54,
        ),
        (
            "\
local counter
counter = 1",
            LuaTarget::Lua54,
        ),
        // `x` is a field of the local `_ENV`.
        (
            "\
local _ENV = {}
x = 1",
            LuaTarget::Lua54,
        ),
        // `global *` declares every free name.
        (
            "\
global *
x = 1",
            LuaTarget::Lua55,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] =
        &[("counter = 1", LuaTarget::Lua54), ("my_global = 1", LuaTarget::Lua54)];

    #[test]
    fn test() {
        Tester::new(RuleId::SettingGlobal, PASS, FAIL).test_and_snapshot();
    }
}
