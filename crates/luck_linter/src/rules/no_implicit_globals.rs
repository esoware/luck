use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Flags every read or write of a global, stdlib names like `print`
/// included, unless it's listed in `extra_globals`.
///
/// This enforces a policy where every dependency arrives through a local or a
/// parameter. Off by default, like every `Style` rule.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NoImplicitGlobals;

declare_lint_rule!(
    NoImplicitGlobals,
    Style,
    Warning,
    "implicit global reference, including standard-library globals"
);

impl Rule for NoImplicitGlobals {
    fn run_once(&self, context: &LintContext<'_, '_>) {
        let semantic = context.semantic;
        let tree = semantic.scoping();

        for reference in tree.global_references() {
            let name = tree.name(reference.name);
            // `_` is a conventional discard name, not a real global.
            if name == "_" {
                continue;
            }
            if semantic.is_extra_global(reference.name) {
                continue;
            }

            context.diagnostic(
                Finding::new(format!("global variable `{name}` used"), reference.span)
                    .with_help("pass it in explicitly, or add it to `extra_globals`"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget, Option<&str>, Option<&str>)] = &[
        (
            "\
local print = function() end
print(\"x\")",
            LuaTarget::Lua54,
            None,
            None,
        ),
        (
            "myCustomGlobal()",
            LuaTarget::Lua54,
            None,
            Some(r#"{"extra_globals": ["myCustomGlobal"]}"#),
        ),
        ("_ = 1", LuaTarget::Lua54, None, None),
    ];

    const FAIL: &[(&str, LuaTarget, Option<&str>, Option<&str>)] = &[
        (r#"print("x")"#, LuaTarget::Lua54, None, None),
        ("myCustomGlobal()", LuaTarget::Lua54, None, None),
        (
            "\
print(\"x\")
myCustomGlobal()",
            LuaTarget::Lua54,
            None,
            Some(r#"{"extra_globals": ["myCustomGlobal"]}"#),
        ),
        ("g = 1", LuaTarget::Lua54, None, None),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::NoImplicitGlobals, PASS, FAIL).test_and_snapshot();
    }
}
