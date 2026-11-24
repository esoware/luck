use luck_ast::{AstKind, AstType, ChainHead, Expression, Suffix};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::Span;

use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::rule::{Rule, declare_lint_rule};

/// Indexing `Enum` with an unknown name throws at runtime on Roblox, so a
/// typo here is always a bug. Only the first segment that fails to resolve is
/// reported; accesses built on a broken prefix stay silent.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RobloxUnknownEnumMember;

declare_lint_rule!(
    RobloxUnknownEnumMember,
    Correctness,
    Error,
    "unknown Roblox enum type, item, or item member"
);

impl Rule for RobloxUnknownEnumMember {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::SuffixedExpression]);

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
            chain.suffixes.len() - 1
        } else {
            chain.suffixes.len()
        };
        for (read_index, suffix) in (0..).zip(chain.suffixes.iter().take(read_count)) {
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
    let path =
        prefix.dotted_path().expect("every segment before the unknown one is a field of `Enum`");
    let message = match path.segment_count() {
        1 => format!("unknown enum type `Enum.{last_segment}`"),
        2 => format!("`{last_segment}` is not an item of `{path}`"),
        _ => format!("`{last_segment}` is not a member of `{path}`"),
    };
    Finding::new(message, span)
        .with_help("enum data is generated from the Roblox API dump, so check the spelling")
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("local v = Enum.Material.Grass.Value", LuaTarget::LuauRoblox),
        (
            "\
local Enum = { Materialz = 1 }
local m = Enum.Materialz",
            LuaTarget::LuauRoblox,
        ),
        ("local m = Enum.Materialz", LuaTarget::Luau),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("local m = Enum.Materialz", LuaTarget::LuauRoblox),
        ("local m = Enum.Material.Grassz", LuaTarget::LuauRoblox),
        ("local n = Enum.Material.Grass.Nome", LuaTarget::LuauRoblox),
        // Only the innermost failing access reports; the outer one stays silent
        // instead of cascading.
        ("local m = Enum.Materialz.Grass", LuaTarget::LuauRoblox),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RobloxUnknownEnumMember, PASS, FAIL).test_and_snapshot();
    }
}
