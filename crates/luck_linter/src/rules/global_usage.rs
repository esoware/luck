use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Flags reads and writes of the global environment tables. `_G` exists in
/// every dialect; `shared` is a Roblox global and an ordinary user name
/// anywhere else, so it only counts under the Roblox stdlib.
pub struct GlobalUsage;

impl Rule for GlobalUsage {
    fn name(&self) -> &'static str {
        "global_usage"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "use of the global environment tables `_G` or `shared`"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let is_roblox = ctx.semantic.environment.is_roblox();
        ctx.semantic
            .scope_tree
            .unresolved_references()
            .filter(|reference| match reference.name.as_str() {
                "_G" => true,
                // Roblox.
                "shared" => is_roblox,
                _ => false,
            })
            .map(|reference| {
                LintDiagnostic::new(
                    self.name(),
                    format!("global environment `{}` used", reference.name),
                    reference.span,
                )
                .with_help("prefer a module or an explicitly passed dependency".to_string())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    #[test]
    fn flags_global_environment_reads_and_writes() {
        let diags = crate::test_support::run_rule_roblox(
            &GlobalUsage,
            "_G.value = shared.value; shared = {}",
        );
        assert_eq!(diags.len(), 3, "{diags:?}");
    }

    #[test]
    fn flags_global_env_table_outside_roblox() {
        let diags = crate::test_support::run_rule(&GlobalUsage, "_G.value = 1", LuaVersion::Lua54);
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_shared_outside_roblox() {
        let diags = crate::test_support::run_rule(
            &GlobalUsage,
            "local function init() shared = {} end",
            LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_ordinary_globals() {
        let diags = crate::test_support::run_rule(
            &GlobalUsage,
            "print(pairs(items)); custom = 1",
            LuaVersion::Lua54,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_local_shadowing_and_fields() {
        let diags = crate::test_support::run_rule_roblox(
            &GlobalUsage,
            "local _G, shared = {}, {}; _G.x = shared.x; object._G = object.shared",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
