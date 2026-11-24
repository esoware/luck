use luck_ast::{AstKind, AstType, Expression, Statement, Var};
use luck_semantic::{AstNode, AstTypesBitset};

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::rule::{RuleRunner, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct AlmostSwapped;

impl_rule!(
    AlmostSwapped,
    "almost_swapped",
    Correctness,
    Error,
    "looks like a failed variable swap; use `a, b = b, a`"
);

impl RuleRunner for AlmostSwapped {
    fn node_types(&self) -> AstTypesBitset {
        AstTypesBitset::from_types(&[AstType::Block])
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let AstKind::Block(block) = node.kind() else {
            return;
        };
        for (first, second) in block.stmts.iter().zip(block.stmts.iter().skip(1)) {
            if let Some(finding) = failed_swap(first, second) {
                context.diagnostic(finding);
            }
        }
    }
}

fn failed_swap(first_stmt: &Statement<'_>, second_stmt: &Statement<'_>) -> Option<Finding> {
    let (Statement::Assignment(first), Statement::Assignment(second)) = (first_stmt, second_stmt)
    else {
        return None;
    };
    let ([first_target], [first_value], [second_target], [second_value]) =
        (&*first.targets, &*first.values, &*second.targets, &*second.values)
    else {
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

fn var_name<'ast>(var: &Var<'ast>) -> Option<&'ast str> {
    let Var::Name(name) = var else {
        return None;
    };
    Some(name.text.as_str())
}

fn expr_var_name<'ast>(expr: &Expression<'ast>) -> Option<&'ast str> {
    let Expression::Name(name) = expr else {
        return None;
    };
    Some(name.text.as_str())
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
        run_rule(&AlmostSwapped, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_failed_swap() {
        let diags = run("\
local a, b = 1, 2
a = b
b = a");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_failed_swap_in_nested_function() {
        let diags = run("\
local f = function() local a, b = 1, 2
a = b
b = a end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_correct_swap() {
        let diags = run("\
local a, b = 1, 2
a, b = b, a");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_unrelated_assignments() {
        let diags = run("\
local a, b = 1, 2
a = b
b = 3");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_failed_swap_separated_by_semicolon() {
        let diags = run("\
local a, b = 1, 2
a = b; b = a");
        assert_eq!(diags.len(), 1, "a `;` doesn't change the failed swap: {diags:?}");
    }
}
