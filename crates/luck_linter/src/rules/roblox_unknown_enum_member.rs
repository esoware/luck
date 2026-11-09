use luck_ast::{DottedPath, Expression, Var, dotted_path};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Flags dotted accesses under the `Enum` global that name a
/// nonexistent enum type, item, or item member.
///
/// Indexing `Enum` with an unknown name throws at runtime on Roblox, so a
/// typo here is always a bug. Only the first segment that fails to resolve is
/// reported; accesses built on a broken prefix stay silent.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RobloxUnknownEnumMember;

impl_rule!(
    RobloxUnknownEnumMember,
    "roblox_unknown_enum_member",
    Correctness,
    Error,
    "unknown Roblox enum type, item, or item member"
);

impl NodeRule for RobloxUnknownEnumMember {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::Var])
    }
    fn on_expr(&self, expr: &Expression, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let Expression::Var(var @ Var::FieldAccess(_)) = expr else {
            return;
        };
        let stdlib = context.semantic.stdlib();
        // Only the Roblox stdlib has an `Enum` global.
        if !stdlib.globals.contains_key("Enum") {
            return;
        }
        let Some(DottedPath { segments, root_span, leaf_span }) = dotted_path(var) else {
            return;
        };
        let Some((last_segment, prefix)) = segments.split_last() else {
            return;
        };
        if prefix.first() != Some(&"Enum") {
            return;
        }
        // A broken prefix is reported by the inner access, so this node only
        // checks its last segment.
        let Some(prefix_entry) = stdlib.lookup(prefix) else {
            return;
        };
        if stdlib.child(prefix_entry, last_segment).is_some() {
            return;
        }
        if context.semantic.resolves_to_local("Enum", root_span) {
            return;
        }
        let prefix_path = prefix.join(".");
        let message = match segments.len() {
            2 => format!("unknown enum type `Enum.{last_segment}`"),
            3 => format!("`{last_segment}` is not an item of `{prefix_path}`"),
            _ => format!("`{last_segment}` is not a member of `{prefix_path}`"),
        };
        findings.push(
            Finding::new(message, leaf_span).with_help(
                "enum data is generated from the Roblox API dump, so check the spelling",
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::RobloxUnknownEnumMember;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::{run_rule, run_rule_roblox};

    fn run_roblox(source: &str) -> Vec<LintDiagnostic> {
        run_rule_roblox(&RobloxUnknownEnumMember, source)
    }

    #[test]
    fn flags_unknown_enum_type() {
        let diags = run_roblox("local m = Enum.Materialz");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("unknown enum type `Enum.Materialz`"));
    }

    #[test]
    fn flags_unknown_enum_item() {
        let diags = run_roblox("local m = Enum.Material.Grassz");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].message.contains("`Grassz` is not an item of `Enum.Material`"),
            "{diags:?}"
        );
    }

    #[test]
    fn flags_unknown_item_member() {
        let diags = run_roblox("local n = Enum.Material.Grass.Nome");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(
            diags[0].message.contains("`Nome` is not a member of `Enum.Material.Grass`"),
            "{diags:?}"
        );
    }

    #[test]
    fn ignores_valid_enum_chain() {
        let diags = run_roblox("local v = Enum.Material.Grass.Value");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_only_first_unknown_segment() {
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
    fn ignores_standalone_environment() {
        let diags =
            run_rule(&RobloxUnknownEnumMember, "local m = Enum.Materialz", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
