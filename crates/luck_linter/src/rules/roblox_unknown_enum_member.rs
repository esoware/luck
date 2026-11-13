use luck_ast::{Expression, Suffix, SuffixedExpression};
use luck_token::Span;

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{ChainPosition, LintContext, NodeRule, impl_rule};

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
        NodeTypeSet::from_types(&[NodeType::Suffixed])
    }
    fn on_chain(
        &self,
        chain: &SuffixedExpression,
        position: ChainPosition,
        context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        let stdlib = context.semantic.stdlib();
        // Only the Roblox stdlib has an `Enum` global.
        if !stdlib.globals.contains_key("Enum") {
            return;
        }
        let Expression::Name(root) = &chain.prefix else {
            return;
        };
        if root.identifier() != Some("Enum")
            || context.semantic.resolves_to_local("Enum", root.span)
        {
            return;
        }
        // A target's last field is stored to, not read.
        let read_count = match position {
            ChainPosition::Value => chain.suffixes.len(),
            ChainPosition::AssignmentTarget => chain.suffixes.len().saturating_sub(1),
        };
        let mut prefix = vec!["Enum"];
        for suffix in chain.suffixes.iter().take(read_count) {
            let Suffix::Field(name) = suffix else {
                return;
            };
            let Some(segment) = name.identifier() else {
                return;
            };
            // Past the first unknown segment, nothing resolves, so the chain
            // warns once.
            let Some(prefix_entry) = stdlib.lookup(&prefix) else {
                return;
            };
            if stdlib.child(prefix_entry, segment).is_none() {
                findings.push(unknown_member(&prefix, segment, name.span));
                return;
            }
            prefix.push(segment);
        }
    }
}

fn unknown_member(prefix: &[&str], last_segment: &str, span: Span) -> Finding {
    let prefix_path = prefix.join(".");
    let message = match prefix.len() {
        1 => format!("unknown enum type `Enum.{last_segment}`"),
        2 => format!("`{last_segment}` is not an item of `{prefix_path}`"),
        _ => format!("`{last_segment}` is not a member of `{prefix_path}`"),
    };
    Finding::new(message, span)
        .with_help("enum data is generated from the Roblox API dump, so check the spelling")
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
