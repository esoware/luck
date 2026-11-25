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
        host.semantic.variant().is_roblox()
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
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
        if !context.scoping().reaches_host_global(root) {
            return;
        }
        for (read_index, suffix) in (0..).zip(context.read_suffixes(node)) {
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
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[&str] = &["local m = Enum.Materialz"];

    const ROBLOX_PASS: &[&str] = &[
        "local v = Enum.Material.Grass.Value",
        "\
local Enum = { Materialz = 1 }
local m = Enum.Materialz",
        "\
Enum = { Materialz = 1 }
local m = Enum.Materialz",
    ];

    const ROBLOX_FAIL: &[&str] = &[
        "local m = Enum.Materialz",
        "local m = Enum.Material.Grassz",
        "local n = Enum.Material.Grass.Nome",
        // Only the innermost failing access reports; the outer one stays silent
        // instead of cascading.
        "local m = Enum.Materialz.Grass",
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::RobloxUnknownEnumMember, PASS, &[])
            .with_roblox(ROBLOX_PASS, ROBLOX_FAIL)
            .test_and_snapshot();
    }
}
