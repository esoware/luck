use luck_ast::Expression;
use luck_ast::expr::Var;
use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;
use luck_token::TokenKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct AlmostSwapped;

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
        "looks like a failed variable swap (use a, b = b, a)"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let mut checker = SwapChecker {
            diagnostics: Vec::new(),
        };
        checker.visit_block(ctx.block);
        checker.diagnostics
    }
}

struct SwapChecker {
    diagnostics: Vec<LintDiagnostic>,
}

fn var_name(var: &Var) -> Option<&str> {
    match var {
        Var::Name(token) => match &token.kind {
            TokenKind::Identifier(name) => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    }
}

fn expr_var_name(expr: &Expression) -> Option<&str> {
    match expr {
        Expression::Var(var) => var_name(var),
        _ => None,
    }
}

impl<'ast> Visitor<'ast> for SwapChecker {
    fn visit_block(&mut self, block: &'ast Block) {
        for pair in block.stmts.windows(2) {
            let (luck_ast::Statement::Assignment(first), luck_ast::Statement::Assignment(second)) =
                (&pair[0], &pair[1])
            else {
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
            // The `a = b; b = a` pattern: the second assignment reads back
            // the value the first just overwrote, so neither variable ends
            // up swapped.
            if target_a == value_b && target_b == value_a {
                self.diagnostics.push(
                    LintDiagnostic::new(
                        "almost_swapped",
                        format!(
                            "`{target_a} = {value_a}; {target_b} = {value_b}` does not swap; use `{target_a}, {target_b} = {target_b}, {target_a}`"
                        ),
                        first.span.merge(second.span),
                    )
                    .with_help(format!(
                        "use `{target_a}, {target_b} = {target_b}, {target_a}` for simultaneous swap"
                    )),
                );
            }
        }
        self.walk_block(block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&AlmostSwapped, source, LuaVersion::Lua54)
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
