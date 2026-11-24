use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Outside Roblox, `shared` is an ordinary name.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct GlobalUsage;

declare_lint_rule!(
    GlobalUsage,
    Style,
    Warning,
    "use of the global environment tables `_G` or `shared`"
);

impl Rule for GlobalUsage {
    fn run_once(&self, context: &LintContext<'_>) {
        let is_roblox = context.semantic.target().is_roblox();
        let tree = context.scoping();
        for reference in tree.global_references() {
            let name = tree.name(reference.name);
            let is_environment = match name {
                "_G" => true,
                // Roblox.
                "shared" => is_roblox,
                _ => false,
            };
            if is_environment {
                context.diagnostic(
                    Finding::new(format!("global environment `{name}` used"), reference.span)
                        .with_help("prefer a module or an explicitly passed dependency"),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("local function init() shared = {} end", LuaTarget::Lua54),
        ("print(pairs(items)); custom = 1", LuaTarget::Lua54),
        (
            "local _G, shared = {}, {}; _G.x = shared.x; object._G = object.shared",
            LuaTarget::LuauRoblox,
        ),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("_G.value = shared.value; shared = {}", LuaTarget::LuauRoblox),
        ("_G.value = 1", LuaTarget::Lua54),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::GlobalUsage, PASS, FAIL).test_and_snapshot();
    }
}
