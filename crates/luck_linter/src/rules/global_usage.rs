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
    use luck_token::LuaVersion;

    use crate::fixtures::{run_rule, run_rule_roblox};
    use crate::rules::RuleId;

    #[test]
    fn flags_global_environment_reads_and_writes() {
        let diags = run_rule_roblox(RuleId::GlobalUsage, "_G.value = shared.value; shared = {}");
        assert_eq!(diags.len(), 3, "{diags:?}");
    }

    #[test]
    fn flags_global_environment_table_outside_roblox() {
        let diags = run_rule(RuleId::GlobalUsage, "_G.value = 1", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_shared_outside_roblox() {
        let diags = run_rule(
            RuleId::GlobalUsage,
            "local function init() shared = {} end",
            LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_ordinary_globals() {
        let diags =
            run_rule(RuleId::GlobalUsage, "print(pairs(items)); custom = 1", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_local_shadowing_and_fields() {
        let diags = run_rule_roblox(
            RuleId::GlobalUsage,
            "local _G, shared = {}, {}; _G.x = shared.x; object._G = object.shared",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
