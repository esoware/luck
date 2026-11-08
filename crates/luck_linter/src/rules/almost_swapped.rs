use luck_ast::{Block, Expression, Statement, Var, Visitor};

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
        let mut checker = SwapChecker { findings: Vec::new() };
        checker.visit_block(context.block);
        checker.findings
    }
}

struct SwapChecker {
    findings: Vec<Finding>,
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

impl<'ast> Visitor<'ast> for SwapChecker {
    fn visit_block(&mut self, block: &'ast Block) {
        for pair in block.stmts.windows(2) {
            let [Statement::Assignment(first), Statement::Assignment(second)] = pair else {
                continue;
            };
            if first.targets.len() != 1
                || second.targets.len() != 1
                || first.values.len() != 1
                || second.values.len() != 1
            {
                continue;
            }
            let (Some(target_a), Some(value_a), Some(target_b), Some(value_b)) = (
                first.targets.first().and_then(var_name),
                first.values.first().and_then(expr_var_name),
                second.targets.first().and_then(var_name),
                second.values.first().and_then(expr_var_name),
            ) else {
                continue;
            };
            // After `a = b`, `b = a` copies `b` back and the old `a` is lost.
            if target_a == value_b && target_b == value_a {
                self.findings.push(
                    Finding::new(
                        format!("`{target_a} = {value_a}; {target_b} = {value_b}` doesn't swap"),
                        first.span.merge(second.span),
                    )
                    .with_help(format!(
                        "use `{target_a}, {target_b} = {target_b}, {target_a}` to swap both at once"
                    )),
                );
            }
        }
        self.walk_block(block);
    }
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
