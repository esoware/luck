use luck_ast::{Expression, Statement, Var};

use crate::diagnostic::{Category, Finding, Severity};
use crate::rule::{LintContext, Rule, WholeRule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct AlmostSwapped;

impl Rule for AlmostSwapped {
    fn name(&self) -> &'static str {
        "almost_swapped"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "looks like a failed variable swap; use `a, b = b, a`"
    }
}

impl WholeRule for AlmostSwapped {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        context
            .blocks
            .iter()
            .flat_map(|block| block.stmts.windows(2))
            .filter_map(failed_swap)
            .collect()
    }
}

fn failed_swap(pair: &[Statement]) -> Option<Finding> {
    let [Statement::Assignment(first), Statement::Assignment(second)] = pair else {
        return None;
    };
    let ([first_target], [first_value], [second_target], [second_value]) = (
        first.targets.items.as_slice(),
        first.values.items.as_slice(),
        second.targets.items.as_slice(),
        second.values.items.as_slice(),
    ) else {
        return None;
    };
    let target_a = var_name(first_target)?;
    let value_a = expr_var_name(first_value)?;
    let target_b = var_name(second_target)?;
    let value_b = expr_var_name(second_value)?;
    // After `a = b`, `b = a` copies `b` back and the old `a` is lost.
    (target_a == value_b && target_b == value_a).then(|| {
        Finding::new(
            format!("`{target_a} = {value_a}; {target_b} = {value_b}` doesn't swap"),
            first.span.merge(second.span),
        )
        .with_help(format!(
            "use `{target_a}, {target_b} = {target_b}, {target_a}` to swap both at once"
        ))
    })
}

fn var_name(var: &Var) -> Option<&str> {
    let Var::Name(token) = var else {
        return None;
    };
    token.identifier()
}

fn expr_var_name(expr: &Expression) -> Option<&str> {
    let Expression::Var(var) = expr else {
        return None;
    };
    var_name(var)
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&AlmostSwapped, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_failed_swap() {
        let diags = run("local a, b = 1, 2\na = b\nb = a");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_failed_swap_in_nested_function() {
        let diags = run("local f = function() local a, b = 1, 2\na = b\nb = a end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_correct_swap() {
        let diags = run("local a, b = 1, 2\na, b = b, a");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_unrelated_assignments() {
        let diags = run("local a, b = 1, 2\na = b\nb = 3");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
