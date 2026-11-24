use luck_ast::{AstKind, AstType};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::number::number_value;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReversedForLoop;

impl_rule!(
    ReversedForLoop,
    "reversed_for_loop",
    Suspicious,
    Warning,
    "numeric `for` loop counts down without a negative step"
);

impl RuleRunner for ReversedForLoop {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[AstType::NumericFor])
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        if let AstKind::NumericFor(numeric_for) = node.kind()
            && numeric_for.step.is_none()
            && let Some(start) = number_value(&numeric_for.start, context)
            && let Some(limit) = number_value(&numeric_for.limit, context)
            && start > limit
        {
            context.diagnostic(
                Finding::new(
                    format!(
                        "`for` loop from {start} to {limit} without a negative step never runs"
                    ),
                    numeric_for.span,
                )
                .with_help("add a negative step: `for i = start, limit, -1 do`"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
        run_rule(&ReversedForLoop, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_descending_bounds_without_step() {
        let diags = run("for i = 10, 1 do print(i) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags[0].message, "`for` loop from 10 to 1 without a negative step never runs",
            "{diags:?}"
        );
    }

    #[test]
    fn flags_hex_start() {
        let diags = run("for i = 0x10, 1 do print(i) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_negated_bounds() {
        let diags = run("for i = -1, -5 do print(i) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_explicit_step() {
        let diags = run("for i = 10, 1, -1 do print(i) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_ascending_bounds() {
        let diags = run("for i = 1, 10 do print(i) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_literal_bounds() {
        let diags = run("\
local n = 0
for i = 10, n do print(i) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_hex_limit_above_i64_on_luau() {
        let source = "for i = 1, 0xFFFFFFFFFFFFFFFF do print(i) end";
        let diags = run_rule(&ReversedForLoop, source, LuaVersion::Luau);
        assert!(diags.is_empty(), "Luau reads this limit as 1.8e19, not -1: {diags:?}");
    }
}
