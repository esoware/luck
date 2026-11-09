use crate::diagnostic::Finding;
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags reads and writes of `_G`, and of `shared` under the Roblox stdlib.
///
/// Outside Roblox, `shared` is an ordinary name.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GlobalUsage;

impl_rule!(
    GlobalUsage,
    "global_usage",
    Style,
    Warning,
    "use of the global environment tables `_G` or `shared`"
);

impl WholeRule for GlobalUsage {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let is_roblox = context.semantic.environment.is_roblox();
        context
            .semantic
            .scope_tree
            .unresolved_references()
            .filter(|reference| match reference.name.as_str() {
                "_G" => true,
                // Roblox.
                "shared" => is_roblox,
                _ => false,
            })
            .map(|reference| {
                Finding::new(
                    format!("global environment `{}` used", reference.name),
                    reference.span,
                )
                .with_help("prefer a module or an explicitly passed dependency")
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::{run_rule, run_rule_roblox};

    #[test]
    fn flags_global_environment_reads_and_writes() {
        let diags = run_rule_roblox(&GlobalUsage, "_G.value = shared.value; shared = {}");
        assert_eq!(diags.len(), 3, "{diags:?}");
    }

    #[test]
    fn flags_global_environment_table_outside_roblox() {
        let diags = run_rule(&GlobalUsage, "_G.value = 1", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_shared_outside_roblox() {
        let diags =
            run_rule(&GlobalUsage, "local function init() shared = {} end", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_ordinary_globals() {
        let diags = run_rule(&GlobalUsage, "print(pairs(items)); custom = 1", LuaVersion::Lua54);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_local_shadowing_and_fields() {
        let diags = run_rule_roblox(
            &GlobalUsage,
            "local _G, shared = {}, {}; _G.x = shared.x; object._G = object.shared",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
