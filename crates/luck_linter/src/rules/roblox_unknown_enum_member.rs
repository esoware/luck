use luck_ast::{AstKind, AstType, ChainHead, DottedPath, Expression, Suffix};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::Span;

use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::rule::{RuleRunner, impl_rule};

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

impl RuleRunner for RobloxUnknownEnumMember {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[AstType::SuffixedExpression])
    }

    fn should_run(&self, host: &ContextHost<'_>) -> bool {
        host.semantic.target().is_roblox()
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return;
        };
        let Expression::Name(root) = &chain.prefix else {
            return;
        };
        if root.text != "Enum" {
            return;
        }
        let stdlib = context.semantic.stdlib();
        let Some(mut entry) = stdlib.globals.get("Enum") else {
            return;
        };
        if context.scoping().resolves_to_local(root) {
            return;
        }
        // A target's last field is stored to, not read.
        let read_count = if context.is_assignment_target(node) {
            chain.suffixes.len().saturating_sub(1)
        } else {
            chain.suffixes.len()
        };
        for (read_index, suffix) in (0usize..).zip(chain.suffixes.iter().take(read_count)) {
            let Suffix::Field(name) = suffix else {
                return;
            };
            let segment = name.text;
            // Past the first unknown segment, nothing resolves, so the chain
            // warns once.
            let Some(member) = stdlib.child(entry, segment.as_str()) else {
                context.diagnostic(unknown_member(
                    chain.head(read_index),
                    segment.as_str(),
                    name.span,
                ));
                return;
            };
            entry = member;
        }
    }
}

/// `prefix` is the dotted path from `Enum` that has no `last_segment`.
fn unknown_member(prefix: ChainHead<'_>, last_segment: &str, span: Span) -> Finding {
    let path = prefix.dotted_path();
    let prefix_path = path.map(|dotted| dotted.to_string()).unwrap_or_default();
    let message = match path.map_or(0, DottedPath::segment_count) {
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
    use crate::diagnostic::Message;
    use crate::fixtures::{run_rule, run_rule_roblox};

    fn run_roblox(source: &str) -> Vec<Message> {
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
        let diags = run_roblox(
            "\
local Enum = { Materialz = 1 }
local m = Enum.Materialz",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_standalone_environment() {
        let diags =
            run_rule(&RobloxUnknownEnumMember, "local m = Enum.Materialz", LuaVersion::Luau);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
