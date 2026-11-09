use luck_ast::Statement;

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::number::signed_number_value;
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReversedForLoop;

impl_rule!(
    ReversedForLoop,
    "reversed_for_loop",
    Suspicious,
    Warning,
    "numeric `for` loop counts down without a negative step"
);

impl NodeRule for ReversedForLoop {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::NumericFor])
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        let version = context.semantic.version;
        if let Statement::NumericFor(numeric_for) = stmt
            && let Some(start) = signed_number_value(&numeric_for.start, version)
            && let Some(limit) = signed_number_value(&numeric_for.limit, version)
            && start > limit
            && numeric_for.step.is_none()
        {
            findings.push(
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
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
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
        let diags = run("local n = 0\nfor i = 10, n do print(i) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_hex_limit_above_i64_on_luau() {
        let source = "for i = 1, 0xFFFFFFFFFFFFFFFF do print(i) end";
        let diags = run_rule(&ReversedForLoop, source, LuaVersion::Luau);
        assert!(diags.is_empty(), "Luau reads this limit as 1.8e19, not -1: {diags:?}");
    }
}
