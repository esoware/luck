use core::ops::Neg;

use luck_ast::{Expression, Statement};
use luck_token::UnOp;

use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::{NodeType, NodeTypeSet};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReversedForLoop;

impl Rule for ReversedForLoop {
    fn name(&self) -> &'static str {
        "reversed_for_loop"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "numeric `for` loop counts down without a negative step"
    }
}

impl NodeRule for ReversedForLoop {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::NumericFor])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Statement::NumericFor(numeric_for) = stmt
            && let Some(start) = literal_value(&numeric_for.start)
            && let Some(limit) = literal_value(&numeric_for.limit)
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

/// The value of a decimal number literal under any number of unary minuses.
/// Rust's float parser reads the text, so hex literals yield `None`.
fn literal_value(expr: &Expression) -> Option<f64> {
    if let Expression::Number(literal) = expr {
        return literal.text.parse().ok();
    }
    let Expression::UnaryOp(unary) = expr else {
        return None;
    };
    if unary.op != UnOp::Neg {
        return None;
    }
    literal_value(&unary.operand).map(Neg::neg)
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
}
