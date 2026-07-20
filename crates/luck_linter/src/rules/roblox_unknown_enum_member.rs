use luck_ast::Expression;
use luck_ast::expr::Var;
use luck_ast::node::{AstTypesBitset, NodeType};

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

/// Flags dotted accesses under the `Enum` global that name a
/// nonexistent enum type, item, or item member. Indexing `Enum` with an
/// unknown name throws at runtime on Roblox, so a typo here is always a
/// bug. Each access reports at the first segment that fails to resolve;
/// nested accesses stay silent when their prefix is already broken.
pub struct RobloxUnknownEnumMember;

impl Rule for RobloxUnknownEnumMember {
    fn name(&self) -> &'static str {
        "roblox_unknown_enum_member"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "unknown Roblox enum type, item, or item member"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

impl NodeRule for RobloxUnknownEnumMember {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::Var]);
        Some(&TYPES)
    }

    fn on_expression(&self, expr: &Expression, ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        let Expression::Var(Var::FieldAccess(_)) = expr else {
            return;
        };
        let Some((segments, spans)) = crate::path::dotted_path(expr) else {
            return;
        };
        if segments[0] != "Enum" {
            return;
        }
        let lib = ctx.semantic.stdlib();
        // Standalone Luau and numbered Lua have no Enum global; the rule
        // only speaks for environments that ship one.
        if !lib.globals.contains_key("Enum") {
            return;
        }
        if ctx.semantic.resolves_to_local("Enum", spans[0]) {
            return;
        }
        let (last, prefix) = segments.split_last().expect("path has >= 2 segments");
        // A broken prefix is reported by the inner access node; this
        // node only owns its final segment.
        let Some(prefix_entry) = lib.lookup_str(prefix) else {
            return;
        };
        if lib.child(prefix_entry, last).is_some() {
            return;
        }
        let prefix_path = prefix.join(".");
        let message = match segments.len() {
            2 => format!("unknown enum type `Enum.{last}`"),
            3 => format!("`{last}` is not an item of `{prefix_path}`"),
            _ => format!("`{last}` is not a member of `{prefix_path}`"),
        };
        out.push(
            LintDiagnostic::new(
                self.name(),
                message,
                *spans.last().expect("span per segment"),
            )
            .with_help(
                "enum data is generated from the Roblox API dump; check the spelling".to_string(),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::RobloxUnknownEnumMember;
    use luck_token::LuaVersion;

    fn run_roblox(source: &str) -> Vec<crate::diagnostic::LintDiagnostic> {
        crate::test_support::run_rule_roblox(&RobloxUnknownEnumMember, source)
    }

    #[test]
    fn flags_unknown_enum_type() {
        let diags = run_roblox("local m = Enum.Materialz");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0]
                .message
                .contains("unknown enum type `Enum.Materialz`")
        );
    }

    #[test]
    fn flags_unknown_enum_item() {
        let diags = run_roblox("local m = Enum.Material.Grassz");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0]
                .message
                .contains("`Grassz` is not an item of `Enum.Material`"),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_unknown_item_member() {
        let diags = run_roblox("local n = Enum.Material.Grass.Nome");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0]
                .message
                .contains("`Nome` is not a member of `Enum.Material.Grass`"),
            "{diags:?}"
        );
    }

    #[test]
    fn ignores_valid_enum_chain() {
        let diags = run_roblox("local v = Enum.Material.Grass.Value");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_broken_prefix_beyond_first_failure() {
        // Only the innermost failing access reports; the outer one
        // stays silent instead of cascading.
        let diags = run_roblox("local m = Enum.Materialz.Grass");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("unknown enum type"));
    }

    #[test]
    fn ignores_shadowed_enum_global() {
        let diags = run_roblox("local Enum = { Materialz = 1 }\nlocal m = Enum.Materialz");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_on_standalone_luau() {
        let diags = crate::test_support::run_rule(
            &RobloxUnknownEnumMember,
            "local m = Enum.Materialz",
            LuaVersion::Luau,
        );
        assert!(diags.is_empty(), "{diags:?}");
    }
}
